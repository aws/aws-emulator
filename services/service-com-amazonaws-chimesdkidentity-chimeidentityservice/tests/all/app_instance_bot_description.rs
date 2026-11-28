// SPDX-License-Identifier: CC0-1.0

//! `DescribeAppInstanceBot` and `UpdateAppInstanceBot`, end to end over HTTP — the bot's
//! read-and-write round trip, so what an update persists is asserted through the reader
//! that reports it.
//!
//! The RULE pinned here: `DescribeAppInstanceBot` REPORTS the bot persisted under the
//! `AppInstanceBotArn` it is asked about — that ARN, the `Name`, `Metadata` and
//! `Configuration` the bot was created with, and the instants it was created and last
//! written at — and changes nothing; an `AppInstanceBotArn` this service resolves no bot
//! for is REFUSED with the 403 `ForbiddenException`, `Code` `Forbidden`, carrying the
//! fixed forbidden-request sentence and naming no ARN, however that ARN came to be
//! unresolvable — so the read discloses nothing about whether such a bot exists.
//!
//! The tables span what the rule generalises over: which optional members a bot carries
//! and at which sizes (absent, empty, the model's length maxima, non-ASCII), the shape of
//! its `Configuration` (the deprecated `RespondsTo` against each `InvokedBy` combination,
//! several locales, with and without a welcome intent), and every way an ARN can fail to
//! resolve to a bot (never minted, under an absent parent, deleted, cascaded away, or
//! naming another resource kind entirely).
//!
//! The second RULE pinned here: `UpdateAppInstanceBot` REPLACES the stored `Name` and
//! `Metadata` of the bot its `AppInstanceBotArn` names with the required pair the request
//! carries, replaces the bot's `Configuration` only when the request carries one — leaving
//! the stored configuration in place when it does not — re-stamps the bot's last-updated
//! instant without moving its created instant, and writes to no other bot; an
//! `AppInstanceBotArn` that resolves to no bot persists nothing and is refused with that
//! same permanent 403, whose table of cases is the one the describe's refusal spans.
//!
//! The third RULE pinned here: that refusal is the modelled `ForbiddenException` CLASS on
//! the wire — the shape a REST+JSON client resolves the failure from, in
//! `x-amzn-errortype` and in the body's `__type` — and never the absence class, so a
//! caller branching on the error code takes the refusal branch for every unresolvable bot
//! ARN.

use serde_json::{json, Value};

use crate::app_instance_lifecycle::{uri_encode, Emulator};

/// The one sentence this service refuses a request this caller may not make with.
const FORBIDDEN_MESSAGE: &str = "You are not allowed to make this request.";

/// A well-formed Amazon Lex V2 bot-alias ARN for `bot_id`/`alias_id`.
///
/// Source: `smithy_query(query="shape",shape="LexConfiguration")` — `LexBotAliasArn` is
/// `@required`, len 15..2048 and
/// `^arn:aws:lex:[a-z]{2}-[a-z]+-\d{1}:\d{12}:bot-alias/[A-Z0-9]{10}/[A-Z0-9]{10}$`.
fn lex_bot_alias_arn(bot_id: &str, alias_id: &str) -> String {
    format!("arn:aws:lex:us-west-2:123456789012:bot-alias/{bot_id}/{alias_id}")
}

/// A `Configuration` driven by the deprecated `RespondsTo` member.
///
/// Source: `smithy_query(query="shape",shape="RespondsTo")` — the enum's only value is
/// `STANDARD_MESSAGES`. `LocaleId` is `@required` and modelled as a free string the Lex V2
/// service resolves, so a locale is a value this table varies rather than a catalog.
fn responds_to_configuration(bot_id: &str, locale_id: &str) -> Value {
    json!({
        "Lex": {
            "LexBotAliasArn": lex_bot_alias_arn(bot_id, "ALIASAAAAA"),
            "LocaleId": locale_id,
            "RespondsTo": "STANDARD_MESSAGES",
        }
    })
}

/// A `Configuration` driven by `InvokedBy`, the member that replaces `RespondsTo`.
///
/// Source: `smithy_query(query="shape",shape="InvokedBy")` — both members are
/// `@required`; `StandardMessages` is one of `AUTO`/`ALL`/`MENTIONS`/`NONE` and
/// `TargetedMessages` one of `ALL`/`NONE`. `WelcomeIntent` is optional, len 1..100,
/// pattern `^([A-Za-z]_?)+$`.
fn invoked_by_configuration(
    bot_id: &str,
    locale_id: &str,
    standard: &str,
    targeted: &str,
    welcome_intent: Option<&str>,
) -> Value {
    let mut lex = json!({
        "LexBotAliasArn": lex_bot_alias_arn(bot_id, "ALIASBBBBB"),
        "LocaleId": locale_id,
        "InvokedBy": { "StandardMessages": standard, "TargetedMessages": targeted },
    });
    if let Some(intent) = welcome_intent {
        lex["WelcomeIntent"] = json!(intent);
    }
    json!({ "Lex": lex })
}

/// `POST /app-instance-bots`, with only the members the row carries, returning the ARN
/// the create minted.
async fn create_bot(
    emulator: &Emulator,
    app_instance_arn: &str,
    token: &str,
    name: Option<&str>,
    metadata: Option<&str>,
    configuration: &Value,
) -> String {
    let mut body = json!({
        "AppInstanceArn": app_instance_arn,
        "ClientRequestToken": token,
        "Configuration": configuration,
    });
    if let Some(name) = name {
        body["Name"] = json!(name);
    }
    if let Some(metadata) = metadata {
        body["Metadata"] = json!(metadata);
    }
    let (status, body) = emulator.call("POST", "/app-instance-bots", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {token:?}: expected CreateAppInstanceBot to answer 201, got {status} {body:?}"
    );
    body["AppInstanceBotArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {token:?}: CreateAppInstanceBot returned no ARN: {body:?}"))
        .to_string()
}

/// `GET /app-instance-bots/{AppInstanceBotArn}`.
async fn describe_bot(emulator: &Emulator, app_instance_bot_arn: &str) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!(
                "/app-instance-bots/{}",
                uri_encode(app_instance_bot_arn)
            ),
            None,
        )
        .await
}

/// `DELETE /app-instance-bots/{AppInstanceBotArn}`, failing the test if it was refused.
async fn delete_bot(emulator: &Emulator, app_instance_bot_arn: &str) {
    let (status, body) = emulator
        .call(
            "DELETE",
            &format!(
                "/app-instance-bots/{}",
                uri_encode(app_instance_bot_arn)
            ),
            None,
        )
        .await;
    assert_eq!(
        status, 204,
        "expected DeleteAppInstanceBot to answer 204, got {status} {body:?}"
    );
}

/// `DELETE /app-instances/{AppInstanceArn}`, failing the test if it was refused.
async fn delete_app_instance(emulator: &Emulator, app_instance_arn: &str) {
    let (status, body) = emulator
        .call(
            "DELETE",
            &format!("/app-instances/{}", uri_encode(app_instance_arn)),
            None,
        )
        .await;
    assert_eq!(
        status, 204,
        "expected DeleteAppInstance to answer 204, got {status} {body:?}"
    );
}

/// `arn` with its REGION segment (the 4th) replaced, so a row can name the same resource
/// in another region without hardcoding the region the emulator was built in.
fn in_another_region(arn: &str) -> String {
    let mut segments: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(segments.len(), 6, "expected a six-segment ARN, got {arn:?}");
    let other = if segments[3] == "eu-west-1" {
        "us-east-2"
    } else {
        "eu-west-1"
    };
    segments[3] = other;
    segments.join(":")
}

/// `arn` with its ACCOUNT segment (the 5th) replaced, so a row can name the same
/// resource in another account without hardcoding the emulator's account id.
fn in_another_account(arn: &str) -> String {
    let mut segments: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(segments.len(), 6, "expected a six-segment ARN, got {arn:?}");
    let other = if segments[4] == "210987654321" {
        "123456789012"
    } else {
        "210987654321"
    };
    segments[4] = other;
    segments.join(":")
}

/// The reported member, or `Value::Null` when the response omits it.
fn member(described: &Value, name: &str) -> Value {
    described["AppInstanceBot"]
        .get(name)
        .cloned()
        .unwrap_or(Value::Null)
}

/// RULE: a describe reports the bot PERSISTED under the ARN it names — that ARN and the
/// `Name`, `Metadata` and `Configuration` the create stored, member for member, with an
/// absent optional member still absent — plus the instants it was created and last
/// written at.
#[tokio::test]
async fn a_describe_reports_the_bot_that_was_created() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("described-bots", "tok-describe").await;

    // Rows differ in which optional members the bot carries and at which size (absent,
    // empty, the modelled maxima of `Name` len 0..256 and `Metadata` len 0..1024,
    // non-ASCII), and in the shape of the configuration: the deprecated `RespondsTo`
    // against each documented `InvokedBy` combination, a different locale each time, and
    // a welcome intent present or not.
    let max_name = "n".repeat(256);
    let max_metadata = "m".repeat(1024);
    let cases: Vec<(&str, Option<&str>, Option<&str>, Value)> = vec![
        (
            "no Name, no Metadata, RespondsTo",
            None,
            None,
            responds_to_configuration("BOTIDAAAAA", "en_US"),
        ),
        (
            "empty Name and Metadata",
            Some(""),
            Some(""),
            responds_to_configuration("BOTIDBBBBB", "de_DE"),
        ),
        (
            "Name at the modelled maximum",
            Some(max_name.as_str()),
            None,
            invoked_by_configuration("BOTIDCCCCC", "ja_JP", "ALL", "NONE", None),
        ),
        (
            "Metadata at the modelled maximum",
            Some("sized"),
            Some(max_metadata.as_str()),
            invoked_by_configuration("BOTIDDDDDD", "fr_FR", "AUTO", "ALL", Some("Welcome")),
        ),
        (
            "non-ASCII Name and Metadata",
            Some("ボット ⚙"),
            Some("メタデータ — ©"),
            invoked_by_configuration(
                "BOTIDEEEEE",
                "es_US",
                "MENTIONS",
                "NONE",
                Some("Greet_User"),
            ),
        ),
        (
            "a bot nothing triggers",
            Some("silent"),
            Some("triggers nothing"),
            invoked_by_configuration("BOTIDFFFFF", "it_IT", "NONE", "NONE", None),
        ),
    ];

    let mut minted: Vec<(&str, String)> = Vec::new();
    for (index, (label, name, metadata, configuration)) in cases.iter().enumerate() {
        let arn = create_bot(
            &emulator,
            &instance,
            &format!("tok-described-{index}"),
            *name,
            *metadata,
            configuration,
        )
        .await;

        let (status, body) = describe_bot(&emulator, &arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected DescribeAppInstanceBot to answer 200, got {status} {body:?}"
        );
        assert_eq!(
            member(&body, "AppInstanceBotArn").as_str(),
            Some(arn.as_str()),
            "case {label:?}: expected the described ARN {arn:?}, got {body:?}"
        );
        assert_eq!(
            member(&body, "Name"),
            name.map(|name| json!(name)).unwrap_or(Value::Null),
            "case {label:?}: expected the Name the create stored ({name:?}), got {body:?}"
        );
        assert_eq!(
            member(&body, "Metadata"),
            metadata.map(|value| json!(value)).unwrap_or(Value::Null),
            "case {label:?}: expected the Metadata the create stored ({metadata:?}), got {body:?}"
        );
        assert_eq!(
            &member(&body, "Configuration"),
            configuration,
            "case {label:?}: expected the Configuration the create stored, got {body:?}"
        );

        let created = member(&body, "CreatedTimestamp").as_f64().unwrap_or_else(|| {
            panic!("case {label:?}: expected a numeric CreatedTimestamp, got {body:?}")
        });
        let updated = member(&body, "LastUpdatedTimestamp")
            .as_f64()
            .unwrap_or_else(|| {
                panic!("case {label:?}: expected a numeric LastUpdatedTimestamp, got {body:?}")
            });
        assert!(
            created > 0.0,
            "case {label:?}: expected a CreatedTimestamp after the epoch, got {created}"
        );
        assert!(
            updated >= created,
            "case {label:?}: expected LastUpdatedTimestamp {updated} at or after CreatedTimestamp {created}"
        );

        minted.push((label, arn));
    }

    // Every ARN still describes ITS OWN bot once all of them exist: a describe resolves
    // the ARN it was given rather than the last bot written.
    for (index, (label, arn)) in minted.iter().enumerate() {
        let (_, _, expected_metadata, expected_configuration) = &cases[index];
        let (status, body) = describe_bot(&emulator, arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the describe to still answer 200, got {status} {body:?}"
        );
        assert_eq!(
            member(&body, "AppInstanceBotArn").as_str(),
            Some(arn.as_str()),
            "case {label:?}: expected the describe to report {arn:?}, got {body:?}"
        );
        assert_eq!(
            member(&body, "Metadata"),
            expected_metadata
                .map(|value| json!(value))
                .unwrap_or(Value::Null),
            "case {label:?}: expected its own Metadata, got {body:?}"
        );
        assert_eq!(
            &member(&body, "Configuration"),
            expected_configuration,
            "case {label:?}: expected its own Configuration, got {body:?}"
        );
    }
}

/// RULE: a describe REPORTS, so repeating it answers exactly the same bot and leaves
/// every later caller — another describe, the listing — seeing what it saw before.
#[tokio::test]
async fn a_describe_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("stable-bots", "tok-stable").await;

    // Rows differ in how many times the same bot is read before the state is checked.
    for (label, reads) in [("read once", 1_usize), ("read twice", 2), ("read ten times", 10)] {
        let arn = create_bot(
            &emulator,
            &instance,
            &format!("tok-stable-{reads}"),
            Some(label),
            Some("unchanged by reading"),
            &responds_to_configuration("BOTIDSTABL", "en_GB"),
        )
        .await;

        let (status, first) = describe_bot(&emulator, &arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the first describe to answer 200, got {status} {first:?}"
        );
        for read in 1..reads {
            let (status, again) = describe_bot(&emulator, &arn).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected read {read} to answer 200, got {status} {again:?}"
            );
            assert_eq!(
                again, first,
                "case {label:?}: expected read {read} to answer exactly what the first did, got {again:?}"
            );
        }

        // The bot is still listed, and still described, after the reads.
        let (status, listed) = emulator
            .call(
                "GET",
                &format!(
                    "/app-instance-bots?app-instance-arn={}",
                    uri_encode(&instance)
                ),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected ListAppInstanceBots to answer 200, got {status} {listed:?}"
        );
        let holds = listed["AppInstanceBots"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: expected a bot list, got {listed:?}"))
            .iter()
            .any(|bot| bot["AppInstanceBotArn"].as_str() == Some(arn.as_str()));
        assert!(
            holds,
            "case {label:?}: expected the read bot {arn:?} to still be listed, got {listed:?}"
        );
    }
}

/// RULE: an `AppInstanceBotArn` this service resolves no bot for reports no bot and is
/// refused as a request this caller may not make — the 403 `ForbiddenException`, `Code`
/// `Forbidden`, carrying the fixed forbidden-request sentence — whatever left that ARN
/// unresolvable, an ARN at or under a deleted `AppInstance` included.
#[tokio::test]
async fn an_unresolvable_bot_arn_is_refused() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("refused-bots", "tok-refused").await;
    let user = emulator.user(&instance, "refused-user", "tok-refused-user").await;
    let kept = create_bot(
        &emulator,
        &instance,
        "tok-refused-kept",
        Some("kept"),
        None,
        &responds_to_configuration("BOTIDKEPTA", "en_US"),
    )
    .await;

    // A bot created and then deleted, and a bot whose parent instance is deleted under
    // it: both ARNs were once real, so they test the refusal and not just a malformed id.
    let deleted = create_bot(
        &emulator,
        &instance,
        "tok-refused-deleted",
        Some("deleted"),
        None,
        &responds_to_configuration("BOTIDDELET", "en_US"),
    )
    .await;
    delete_bot(&emulator, &deleted).await;

    let doomed_instance = emulator.app_instance("doomed", "tok-refused-doomed").await;
    let cascaded = create_bot(
        &emulator,
        &doomed_instance,
        "tok-refused-cascaded",
        Some("cascaded"),
        None,
        &responds_to_configuration("BOTIDCASCA", "en_US"),
    )
    .await;
    delete_app_instance(&emulator, &doomed_instance).await;

    // Rows span every way a well-formed ChimeArn can fail to resolve to a bot; every one
    // of them earns the SAME answer, the permanent refusal, so the row's class cannot be
    // read back off the response.
    for (label, arn) in [
        (
            "an ARN under a live parent that was never minted",
            format!("{instance}/bot/never-minted"),
        ),
        (
            "a bot ARN under an AppInstance that does not exist",
            format!("{instance}x/bot/absent-parent"),
        ),
        ("a bot that was deleted", deleted.clone()),
        ("a bot whose parent AppInstance was deleted", cascaded.clone()),
        ("the parent AppInstance's own ARN", instance.clone()),
        ("an AppInstanceUser ARN", user.clone()),
        (
            "the same bot ARN in another account",
            in_another_account(&kept),
        ),
    ] {
        let (status, body) = describe_bot(&emulator, &arn).await;
        assert_eq!(
            status, 403,
            "case {label:?}: expected {arn:?} to be refused with 403, got {status} {body:?}"
        );
        assert_eq!(
            body["Code"].as_str(),
            Some("Forbidden"),
            "case {label:?}: expected the Forbidden code, got {body:?}"
        );
        assert_eq!(
            body["Message"].as_str(),
            Some(FORBIDDEN_MESSAGE),
            "case {label:?}: expected the fixed forbidden-request sentence, got {body:?}"
        );
        assert!(
            body.get("AppInstanceBot").is_none(),
            "case {label:?}: expected the refusal to report no bot, got {body:?}"
        );
    }

    // None of the refusals disturbed the bot that does exist.
    let (status, body) = describe_bot(&emulator, &kept).await;
    assert_eq!(
        status, 200,
        "expected the surviving bot to still be described, got {status} {body:?}"
    );
    assert_eq!(
        member(&body, "Name").as_str(),
        Some("kept"),
        "expected the surviving bot to report the Name it was created with, got {body:?}"
    );
}

/// RULE: a `DescribeAppInstanceBot` addressed to an ARN this service resolves no bot for
/// is REFUSED with one answer — the 403 `ForbiddenException`, `Code` `Forbidden`, and the
/// fixed forbidden-request sentence, with nothing else in the body — whatever class of
/// unresolvable ARN it was, so the response body is byte for byte the same for every row
/// and discloses neither the addressed ARN nor whether a bot carries it.
#[tokio::test]
async fn every_unresolvable_bot_arn_earns_the_same_undisclosing_refusal() {
    let emulator = Emulator::new();
    let instance = emulator
        .app_instance("undisclosed-bots", "tok-undisclosed")
        .await;
    let user = emulator
        .user(&instance, "undisclosed-user", "tok-undisclosed-user")
        .await;
    let live = create_bot(
        &emulator,
        &instance,
        "tok-undisclosed-live",
        Some("live"),
        Some("still here"),
        &responds_to_configuration("BOTIDLIVEA", "en_US"),
    )
    .await;

    let deleted = create_bot(
        &emulator,
        &instance,
        "tok-undisclosed-deleted",
        Some("deleted"),
        None,
        &responds_to_configuration("BOTIDGONEA", "en_US"),
    )
    .await;
    delete_bot(&emulator, &deleted).await;

    let doomed_instance = emulator
        .app_instance("undisclosed-doomed", "tok-undisclosed-doomed")
        .await;
    let cascaded = create_bot(
        &emulator,
        &doomed_instance,
        "tok-undisclosed-cascaded",
        Some("cascaded"),
        None,
        &responds_to_configuration("BOTIDCASCB", "en_US"),
    )
    .await;
    delete_app_instance(&emulator, &doomed_instance).await;

    // Rows span the classes the rule generalises over: WHY the ARN resolves to no bot
    // (never minted, minted and deleted, cascaded away with its parent, belonging to
    // another account or region, or naming a live resource that is not a bot — a user,
    // the parent instance itself, or a resource of another service entirely), and the
    // SHAPE of the ARN (one segment longer than a bot ARN, one character off a live
    // bot's, a resource path of its own). Every `ChimeArn` here satisfies the modelled
    // pattern, so what is under test is the refusal and not input validation.
    //
    // Source for the ARN shape: `smithy_query(query="shape",shape="ChimeArn")` — len
    // 5..1600 and `^arn:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{0,63}:[a-z0-9-\.]{0,63}:[a-z0-9-\.]{0,63}:[^/].{0,1023}$`.
    let one_character_off = {
        let mut flipped = live.clone();
        flipped.pop();
        format!("{flipped}z")
    };
    let rows: Vec<(&str, String)> = vec![
        (
            "a bot never minted under a live AppInstance",
            format!("{instance}/bot/never-minted"),
        ),
        (
            "a bot under an AppInstance that never existed",
            format!("{instance}-absent/bot/orphan"),
        ),
        ("a bot that was deleted", deleted.clone()),
        ("a bot whose parent AppInstance was deleted", cascaded.clone()),
        (
            "a live bot's ARN in another account",
            in_another_account(&live),
        ),
        (
            "a live bot's ARN in another region",
            in_another_region(&live),
        ),
        ("a live AppInstanceUser's ARN", user.clone()),
        ("the live parent AppInstance's own ARN", instance.clone()),
        (
            "an ARN one character off a live bot's",
            one_character_off.clone(),
        ),
        (
            "a bot ARN with a sub-resource path of its own",
            format!("{live}/channel/extra"),
        ),
        (
            "an ARN of another AWS service entirely",
            "arn:aws:s3:us-west-2:123456789012:bucket-that-is-not-a-bot".to_string(),
        ),
    ];

    // The refusal is ONE answer across the classes: the body is the same for every row,
    // so nothing about the addressed ARN leaks through it.
    let mut canonical_refusal: Option<(&str, Value)> = None;
    for (label, arn) in &rows {
        let (status, body) = describe_bot(&emulator, arn).await;
        assert_eq!(
            status, 403,
            "case {label:?}: expected {arn:?} to be refused with 403, got {status} {body:?}"
        );
        assert!(
            body.get("AppInstanceBot").is_none(),
            "case {label:?}: expected the answer to report no bot, got {body:?}"
        );
        assert_eq!(
            body["Code"].as_str(),
            Some("Forbidden"),
            "case {label:?}: expected the Forbidden code, got {body:?}"
        );
        assert_eq!(
            body["Message"].as_str(),
            Some(FORBIDDEN_MESSAGE),
            "case {label:?}: expected the fixed forbidden-request sentence, got {body:?}"
        );
        assert!(
            !body.to_string().contains(arn.as_str()),
            "case {label:?}: expected the refusal to mention {arn:?} nowhere, got {body:?}"
        );
        match &canonical_refusal {
            None => canonical_refusal = Some((label, body.clone())),
            Some((first_label, first_body)) => assert_eq!(
                &body, first_body,
                "case {label:?}: expected the same refusal as case {first_label:?} \
                 ({first_body:?}), got {body:?}"
            ),
        }
    }

    // The refusals are not a blanket one: the bot that does exist is still reported, with
    // everything it was created with, after every row above.
    let (status, body) = describe_bot(&emulator, &live).await;
    assert_eq!(
        status, 200,
        "expected the live bot to still be described, got {status} {body:?}"
    );
    assert_eq!(
        member(&body, "AppInstanceBotArn").as_str(),
        Some(live.as_str()),
        "expected the live bot to report its own ARN, got {body:?}"
    );
    assert_eq!(
        member(&body, "Metadata").as_str(),
        Some("still here"),
        "expected the live bot to report the Metadata it was created with, got {body:?}"
    );
}

/// `PUT /app-instance-bots/{AppInstanceBotArn}` with the `Name`/`Metadata` pair the model
/// requires, and a `Configuration` only when the row carries one.
///
/// Source: `smithy_query(query="operation",operation="UpdateAppInstanceBot")` — `PUT
/// /app-instance-bots/{AppInstanceBotArn}`, `Name` (len 0..256) and `Metadata` (len
/// 0..1024) `@required`, `Configuration` optional, answering 200 with the bot's ARN.
async fn update_bot(
    emulator: &Emulator,
    app_instance_bot_arn: &str,
    name: &str,
    metadata: &str,
    configuration: Option<&Value>,
) -> (u16, Value) {
    let mut body = json!({ "Name": name, "Metadata": metadata });
    if let Some(configuration) = configuration {
        body["Configuration"] = configuration.clone();
    }
    emulator
        .call(
            "PUT",
            &format!("/app-instance-bots/{}", uri_encode(app_instance_bot_arn)),
            Some(body),
        )
        .await
}

/// The summary `ListAppInstanceBots` reports for `bot_arn` under `app_instance_arn`, or
/// `Value::Null` when the listing holds no row for it.
async fn listed_bot(emulator: &Emulator, app_instance_arn: &str, bot_arn: &str) -> Value {
    let (status, body) = emulator
        .call(
            "GET",
            &format!(
                "/app-instance-bots?app-instance-arn={}",
                uri_encode(app_instance_arn)
            ),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "expected ListAppInstanceBots to answer 200, got {status} {body:?}"
    );
    body["AppInstanceBots"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a bot list, got {body:?}"))
        .iter()
        .find(|bot| bot["AppInstanceBotArn"].as_str() == Some(bot_arn))
        .cloned()
        .unwrap_or(Value::Null)
}

/// RULE: an update REPLACES the stored `Name`/`Metadata` pair of the bot its ARN names,
/// replaces the `Configuration` only when the request carries one and otherwise leaves the
/// stored configuration in place, re-stamps the last-updated instant without moving the
/// created instant, answers with that bot's ARN, and writes to no other bot.
#[tokio::test]
async fn an_update_replaces_the_stored_pair_and_only_the_configuration_it_is_given() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("updated-bots", "tok-update").await;

    let created_configuration = responds_to_configuration("BOTIDUPDAT", "en_US");
    let target = create_bot(
        &emulator,
        &instance,
        "tok-update-target",
        Some("created-name"),
        Some("created-metadata"),
        &created_configuration,
    )
    .await;
    // A second bot under the same parent: every row below has to leave it exactly as its
    // create left it, so an update writes to the ONE bot its ARN names.
    let bystander = create_bot(
        &emulator,
        &instance,
        "tok-update-bystander",
        Some("bystander"),
        Some("untouched"),
        &invoked_by_configuration("BOTIDBYSTA", "en_GB", "ALL", "ALL", Some("Hello")),
    )
    .await;
    let (status, bystander_before) = describe_bot(&emulator, &bystander).await;
    assert_eq!(
        status, 200,
        "expected the bystander bot to describe, got {status} {bystander_before:?}"
    );

    let (status, before) = describe_bot(&emulator, &target).await;
    assert_eq!(
        status, 200,
        "expected the fresh bot to describe, got {status} {before:?}"
    );
    let created = member(&before, "CreatedTimestamp")
        .as_f64()
        .unwrap_or_else(|| panic!("expected a numeric CreatedTimestamp, got {before:?}"));
    let mut previous_update = member(&before, "LastUpdatedTimestamp")
        .as_f64()
        .unwrap_or_else(|| panic!("expected a numeric LastUpdatedTimestamp, got {before:?}"));

    // Rows differ along what the rule generalises over: the value and size of the pair
    // being written (ordinary text, the empty values the model's `min 0` lengths allow,
    // each member at its modelled maximum, non-ASCII text), and whether the request
    // carries a `Configuration` at all — one driven by the deprecated `RespondsTo`, one by
    // `InvokedBy`, and rows carrying none, which must keep whatever the bot already holds.
    //
    // Source: `smithy_query(query="operation",operation="UpdateAppInstanceBot")` — `Name`
    // is `ResourceName` (len 0..256) and `Metadata` is len 0..1024.
    let max_name = "n".repeat(256);
    let max_metadata = "m".repeat(1024);
    let rows: Vec<(&str, String, String, Option<Value>)> = vec![
        (
            "an ordinary rename, no Configuration",
            "renamed".to_string(),
            "metadata-1".to_string(),
            None,
        ),
        (
            "the empty Name and Metadata the model allows",
            String::new(),
            String::new(),
            None,
        ),
        (
            "Name at the modelled maximum",
            max_name.clone(),
            "metadata-2".to_string(),
            None,
        ),
        (
            "Metadata at the modelled maximum",
            "sized".to_string(),
            max_metadata.clone(),
            None,
        ),
        (
            "non-ASCII Name and Metadata",
            "ボット ⚙".to_string(),
            "メタデータ — ©".to_string(),
            None,
        ),
        (
            "a Configuration driven by InvokedBy",
            "reconfigured".to_string(),
            "metadata-3".to_string(),
            Some(invoked_by_configuration(
                "BOTIDNEWCF",
                "de_DE",
                "MENTIONS",
                "ALL",
                Some("Greet_User"),
            )),
        ),
        (
            "a Configuration driven by RespondsTo",
            "reconfigured-again".to_string(),
            String::new(),
            Some(responds_to_configuration("BOTIDOLDCF", "ja_JP")),
        ),
        (
            "no Configuration after one was replaced",
            "pair-only".to_string(),
            "metadata-4".to_string(),
            None,
        ),
    ];

    let mut expected_configuration = created_configuration.clone();
    for (label, name, metadata, configuration) in &rows {
        let (status, answered) =
            update_bot(&emulator, &target, name, metadata, configuration.as_ref()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected UpdateAppInstanceBot to answer 200, got {status} \
             {answered:?}"
        );
        assert_eq!(
            answered["AppInstanceBotArn"].as_str(),
            Some(target.as_str()),
            "case {label:?}: expected the update to answer with {target:?}, got {answered:?}"
        );
        if let Some(configuration) = configuration {
            expected_configuration = configuration.clone();
        }

        let (status, after) = describe_bot(&emulator, &target).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the updated bot to describe, got {status} {after:?}"
        );
        assert_eq!(
            member(&after, "Name").as_str(),
            Some(name.as_str()),
            "case {label:?}: expected Name {name:?} after the update, got {after:?}"
        );
        assert_eq!(
            member(&after, "Metadata").as_str(),
            Some(metadata.as_str()),
            "case {label:?}: expected Metadata {metadata:?} after the update, got {after:?}"
        );
        assert_eq!(
            member(&after, "Configuration"),
            expected_configuration,
            "case {label:?}: expected the Configuration {expected_configuration:?}, got \
             {after:?}"
        );
        assert_eq!(
            member(&after, "CreatedTimestamp").as_f64(),
            Some(created),
            "case {label:?}: an update must not move the created instant {created}, got \
             {after:?}"
        );
        let updated = member(&after, "LastUpdatedTimestamp").as_f64();
        assert!(
            updated.is_some_and(|at| at >= created && at >= previous_update),
            "case {label:?}: expected a last-updated instant at or after both the created \
             instant {created} and the previous stamp {previous_update}, got {updated:?}"
        );
        previous_update = updated.unwrap_or(previous_update);
    }

    // The listing reports the pair the LAST update wrote, so the update persisted to the
    // one store every reader of this bot reads.
    let (_, final_name, final_metadata, _) = rows.last().expect("a non-empty table");
    let summary = listed_bot(&emulator, &instance, &target).await;
    assert_eq!(
        summary["Name"].as_str(),
        Some(final_name.as_str()),
        "expected the listing to report the last Name written, got {summary:?}"
    );
    assert_eq!(
        summary["Metadata"].as_str(),
        Some(final_metadata.as_str()),
        "expected the listing to report the last Metadata written, got {summary:?}"
    );

    // Not one of those updates touched the other bot of the same AppInstance.
    let (status, bystander_after) = describe_bot(&emulator, &bystander).await;
    assert_eq!(
        (status, &bystander_after),
        (200, &bystander_before),
        "expected the bystander bot to be exactly what its create left, got \
         {bystander_after:?}"
    );
}

/// RULE: an `UpdateAppInstanceBot` addressed to an `AppInstanceBotArn` this service
/// resolves no bot for persists nothing and is refused as a request this caller may not
/// make — the permanent 403 `ForbiddenException` carrying that refusal's one fixed sentence
/// — whichever way the ARN failed to resolve.
#[tokio::test]
async fn updating_an_arn_this_service_holds_no_bot_for_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let instance = emulator
        .app_instance("unwritable-bots", "tok-update-absent")
        .await;
    let user = emulator
        .user(&instance, "update-absent-user", "tok-update-absent-user")
        .await;
    let kept = create_bot(
        &emulator,
        &instance,
        "tok-update-kept",
        Some("kept"),
        Some("kept-metadata"),
        &responds_to_configuration("BOTIDKEPTU", "en_US"),
    )
    .await;
    let (status, kept_before) = describe_bot(&emulator, &kept).await;
    assert_eq!(
        status, 200,
        "expected the kept bot to describe, got {status} {kept_before:?}"
    );

    let deleted = create_bot(
        &emulator,
        &instance,
        "tok-update-deleted",
        Some("deleted"),
        None,
        &responds_to_configuration("BOTIDDELEU", "en_US"),
    )
    .await;
    delete_bot(&emulator, &deleted).await;

    let doomed_instance = emulator
        .app_instance("update-doomed", "tok-update-doomed")
        .await;
    let cascaded = create_bot(
        &emulator,
        &doomed_instance,
        "tok-update-cascaded",
        Some("cascaded"),
        None,
        &responds_to_configuration("BOTIDCASCU", "en_US"),
    )
    .await;
    delete_app_instance(&emulator, &doomed_instance).await;

    // Rows span every way a well-formed `ChimeArn` can fail to name a bot this service
    // holds; each is a request the caller may not make, so each earns the same refusal.
    let rows: Vec<(&str, String)> = vec![
        (
            "a bot never minted under a live AppInstance",
            format!("{instance}/bot/never-minted"),
        ),
        (
            "a bot under an AppInstance that never existed",
            format!("{instance}-absent/bot/orphan"),
        ),
        ("a bot that was deleted", deleted.clone()),
        (
            "a live AppInstanceUser's ARN in the bot space",
            user.clone(),
        ),
        ("the live parent AppInstance's own ARN", instance.clone()),
        (
            "a live bot's ARN in another account",
            in_another_account(&kept),
        ),
        (
            "a live bot's ARN in another region",
            in_another_region(&kept),
        ),
        (
            "a bot whose parent AppInstance was deleted",
            cascaded.clone(),
        ),
        (
            "the deleted AppInstance's own ARN",
            doomed_instance.clone(),
        ),
    ];

    for (label, arn) in &rows {
        let (status, body) = update_bot(&emulator, arn, "written", "written", None).await;
        assert_eq!(
            status, 403,
            "case {label:?}: expected {arn:?} to be refused with 403, got {status} {body:?}"
        );
        assert_eq!(
            body["Code"].as_str(),
            Some("Forbidden"),
            "case {label:?}: expected the Forbidden code, got {body:?}"
        );
        assert!(
            body.get("AppInstanceBotArn").is_none(),
            "case {label:?}: expected the refusal to report no ARN, got {body:?}"
        );
        assert_eq!(
            body["Message"].as_str(),
            Some(FORBIDDEN_MESSAGE),
            "case {label:?}: expected the forbidden-request sentence, got {body:?}"
        );
    }

    // None of the refused writes landed anywhere: the one live bot is byte for byte what
    // its create left, last-updated instant included, and the deleted bot is still gone.
    let (status, kept_after) = describe_bot(&emulator, &kept).await;
    assert_eq!(
        (status, &kept_after),
        (200, &kept_before),
        "expected the live bot to be untouched by the refusals, got {kept_after:?}"
    );
    let (status, body) = describe_bot(&emulator, &deleted).await;
    assert_eq!(
        status, 403,
        "expected the deleted bot to stay unresolvable after the refused update, got \
         {status} {body:?}"
    );
}

/// RULE: the describe's answer tracks whether the addressed bot is in the system — it
/// reports the bot while it is stored and refuses the read with the fixed 403
/// `ForbiddenException` once it is not, whatever the bot carried — and that refusal is the
/// same body for every unresolvable ARN, so it tells a caller nothing about which ARN it
/// asked about.
#[tokio::test]
async fn the_refusal_tracks_whether_the_bot_is_in_the_system() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("absent-bots", "tok-absent").await;
    let neighbour = emulator
        .app_instance("absent-bots-neighbour", "tok-absent-neighbour")
        .await;

    // Rows differ in what the bot carries while it exists — which optional members
    // (none, a `Name`, a `Name` and non-ASCII `Metadata`) and which `Configuration` shape
    // (the deprecated `RespondsTo`, or `InvokedBy` with and without a welcome intent) —
    // and in which of the two live `AppInstance`s owns it, so the answer after the delete
    // is asserted over the class and not over one stored bot.
    let rows: Vec<(&str, Option<&str>, Option<&str>, Value)> = vec![
        (
            "a bot carrying neither Name nor Metadata",
            None,
            None,
            responds_to_configuration("BOTIDBAREA", "en_US"),
        ),
        (
            "a bot carrying only a Name",
            Some("named"),
            None,
            invoked_by_configuration("BOTIDNAMED", "en_GB", "ALL", "NONE", None),
        ),
        (
            "a bot carrying a Name and non-ASCII Metadata",
            Some("rich"),
            Some("メタデータ"),
            invoked_by_configuration("BOTIDRICHA", "de_DE", "MENTIONS", "ALL", Some("Welcome")),
        ),
    ];

    for (owner_index, (owner_label, owner)) in [
        ("the first AppInstance", &instance),
        ("another AppInstance", &neighbour),
    ]
    .iter()
    .enumerate()
    {
        for (row_index, (label, name, metadata, configuration)) in rows.iter().enumerate() {
            let case = format!("{label} under {owner_label}");
            // `ClientRequestToken` is len 2..64, pattern `^[-_a-zA-Z0-9]*$`
            // (`smithy_query(query="shape",shape="ClientRequestToken")`), so each row's
            // token is built from the two indices rather than from its label.
            let token = format!("tok-absent-{owner_index}-{row_index}");
            let arn = create_bot(&emulator, owner, &token, *name, *metadata, configuration).await;

            // While the bot is in the system the describe reports it, so the refusal
            // below is about the bot leaving the store and not about an ARN the service
            // never served.
            let (status, body) = describe_bot(&emulator, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the stored bot to be described, got {status} {body:?}"
            );
            assert_eq!(
                member(&body, "Name").as_str(),
                *name,
                "case {case:?}: expected the stored Name, got {body:?}"
            );
            assert_eq!(
                &member(&body, "Configuration"),
                configuration,
                "case {case:?}: expected the stored Configuration, got {body:?}"
            );

            delete_bot(&emulator, &arn).await;

            // Out of the system: the permanent refusal, its `Code` and the fixed
            // forbidden-request sentence, which names no ARN.
            let (status, body) = describe_bot(&emulator, &arn).await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected the deleted bot to be refused with 403, got \
                 {status} {body:?}"
            );
            assert_eq!(
                body["Code"].as_str(),
                Some("Forbidden"),
                "case {case:?}: expected the Forbidden code, got {body:?}"
            );
            assert_eq!(
                body["Message"].as_str(),
                Some(FORBIDDEN_MESSAGE),
                "case {case:?}: expected the fixed forbidden-request sentence, got {body:?}"
            );
            assert!(
                body.get("AppInstanceBot").is_none(),
                "case {case:?}: expected no bot to be reported, got {body:?}"
            );

            // A second ARN the service resolves no bot for is refused identically, so
            // neither answer carries anything of the ARN it was addressed to.
            let other = format!("{owner}/bot/{token}-never-minted");
            let (other_status, other_body) = describe_bot(&emulator, &other).await;
            assert_eq!(
                (other_status, &other_body),
                (403, &body),
                "case {case:?}: expected {other:?} to earn the same refusal as the deleted \
                 bot ({body:?}), got {other_status} {other_body:?}"
            );
        }
    }
}

/// Which operations are addressed BY an `AppInstanceBotArn`: the ones whose refusal of an
/// ARN that resolves to no bot the rule below is about
/// (`smithy_query(query="digest")` on this crate's model lists the three paths under
/// `/app-instance-bots/{AppInstanceBotArn}`).
const BOT_ADDRESSED_OPERATIONS: [(&str, &str); 3] = [
    ("DescribeAppInstanceBot", "GET"),
    ("UpdateAppInstanceBot", "PUT"),
    ("DeleteAppInstanceBot", "DELETE"),
];

/// RULE: EVERY operation addressed by an `AppInstanceBotArn` refuses an ARN this service
/// resolves no bot for the same way — the permanent 403 `ForbiddenException`, `Code`
/// `Forbidden`, carrying this service's one fixed sentence for a request the caller may not
/// make — whichever way that ARN failed to resolve; the control is the live bot, which every
/// one of those operations serves.
#[tokio::test]
async fn every_operation_addressed_by_a_bot_arn_refuses_an_unresolvable_one_alike() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("bot-arn-class", "tok-arn-class").await;
    let user = emulator
        .user(&instance, "arn-class-user", "tok-arn-class-user")
        .await;

    let deleted = create_bot(
        &emulator,
        &instance,
        "tok-arn-class-deleted",
        Some("deleted"),
        None,
        &responds_to_configuration("BOTIDCLASA", "en_US"),
    )
    .await;
    delete_bot(&emulator, &deleted).await;

    let doomed_instance = emulator
        .app_instance("arn-class-doomed", "tok-arn-class-doomed")
        .await;
    let cascaded = create_bot(
        &emulator,
        &doomed_instance,
        "tok-arn-class-cascaded",
        Some("cascaded"),
        None,
        &responds_to_configuration("BOTIDCLASB", "en_US"),
    )
    .await;
    delete_app_instance(&emulator, &doomed_instance).await;

    // The ways a well-formed `ChimeArn` fails to resolve to a bot: never minted, minted and
    // deleted, cascaded away with its parent `AppInstance`, another account's, another
    // region's, and a live ARN of another resource kind.
    let unresolvable: Vec<(&str, String)> = vec![
        (
            "a bot never minted under a live AppInstance",
            format!("{instance}/bot/never-minted"),
        ),
        (
            "a bot under an AppInstance that never existed",
            format!("{instance}-absent/bot/orphan"),
        ),
        ("a bot that was deleted", deleted.clone()),
        (
            "a bot whose parent AppInstance was deleted",
            cascaded.clone(),
        ),
        (
            "a deleted bot's ARN in another account",
            in_another_account(&deleted),
        ),
        (
            "a deleted bot's ARN in another region",
            in_another_region(&deleted),
        ),
        ("a live AppInstanceUser's ARN", user.clone()),
        ("the live parent AppInstance's own ARN", instance.clone()),
    ];

    for (operation, method) in BOT_ADDRESSED_OPERATIONS {
        for (label, arn) in &unresolvable {
            let case = format!("{operation} on {label}");
            // `UpdateAppInstanceBot` is the one of the three with a body, and its
            // `Name`/`Metadata` pair is `@required`.
            let body = (operation == "UpdateAppInstanceBot")
                .then(|| json!({ "Name": "written", "Metadata": "written" }));
            let (status, answered) = emulator
                .call(
                    method,
                    &format!("/app-instance-bots/{}", uri_encode(arn)),
                    body,
                )
                .await;
            assert_eq!(
                (
                    status,
                    answered["Code"].as_str(),
                    answered["Message"].as_str()
                ),
                (403, Some("Forbidden"), Some(FORBIDDEN_MESSAGE)),
                "case {case:?}: expected the permanent refusal, got {status} {answered:?}"
            );
        }
    }

    // The control: a bot that IS there is served by every one of those operations, so the
    // refusals above are about the ARN resolving to nothing and not a blanket answer.
    let live = create_bot(
        &emulator,
        &instance,
        "tok-arn-class-live",
        Some("live"),
        Some("still here"),
        &responds_to_configuration("BOTIDCLASC", "en_US"),
    )
    .await;
    let (status, described) = describe_bot(&emulator, &live).await;
    assert_eq!(
        status, 200,
        "expected the live bot to be described, got {status} {described:?}"
    );
    let (status, updated) = update_bot(&emulator, &live, "written", "written", None).await;
    assert_eq!(
        status, 200,
        "expected the live bot to take the update, got {status} {updated:?}"
    );
    let (status, removed) = emulator
        .call(
            "DELETE",
            &format!("/app-instance-bots/{}", uri_encode(&live)),
            None,
        )
        .await;
    assert_eq!(
        status, 204,
        "expected the live bot to be deleted, got {status} {removed:?}"
    );
    // And once it is gone its own ARN earns the refusal the rule states.
    let (status, after) = describe_bot(&emulator, &live).await;
    assert_eq!(
        (status, after["Code"].as_str()),
        (403, Some("Forbidden")),
        "expected the deleted bot's own ARN to earn the refusal, got {status} {after:?}"
    );
}

/// RULE: the describe's refusal of an unresolvable `AppInstanceBotArn` is PER-RESOURCE and
/// not a refusal of the caller — the same caller still reads the bot's parent
/// `AppInstance` (200) and still lists that instance's bots (200, with no row for the
/// addressed ARN), while the bot read itself is refused 403 whether the ARN is simply one
/// the parent never minted or one whose parent has since been deleted.
#[tokio::test]
async fn the_bot_read_refusal_leaves_the_parent_instance_readable() {
    let emulator = Emulator::new();
    let instance = emulator
        .app_instance("per-resource-refusal", "tok-per-resource")
        .await;
    let sibling = create_bot(
        &emulator,
        &instance,
        "tok-per-resource-sibling",
        Some("sibling"),
        Some("listed"),
        &responds_to_configuration("BOTIDSIBLI", "en_US"),
    )
    .await;

    // Rows differ in the SHAPE of the bot id the unresolvable ARN carries under the live
    // parent — a uuid, a decimal id, one long enough to leave the uuid shape far behind,
    // and one carrying the separators a resource path uses — so the refusal is asserted
    // over the class and not over one id. Every ARN here stays inside the modelled
    // `ChimeArn` bounds (`smithy_query(query="shape",shape="ChimeArn")`: len 5..1600,
    // pattern `^arn:...:[^/].{0,1023}$`), so what is under test is the refusal and not
    // input validation.
    let long_tail = "b".repeat(64);
    let unresolvable: Vec<(&str, String)> = vec![
        (
            "a uuid-shaped bot id",
            format!("{instance}/bot/3f9c1c7e-2b5a-4f8e-9f1a-26a2f7c0d111"),
        ),
        ("a decimal bot id", format!("{instance}/bot/42")),
        ("a long bot id", format!("{instance}/bot/{long_tail}")),
        (
            "a bot id carrying its own path separators",
            format!("{instance}/bot/team/alpha/assistant"),
        ),
    ];

    for (label, arn) in &unresolvable {
        let (status, body) = describe_bot(&emulator, arn).await;
        assert_eq!(
            (status, body["Code"].as_str(), body["Message"].as_str()),
            (403, Some("Forbidden"), Some(FORBIDDEN_MESSAGE)),
            "case {label:?}: expected {arn:?} to be refused with the fixed forbidden \
             answer, got {status} {body:?}"
        );

        // The controls: the caller that was just refused the bot read still reads the
        // parent instance and still lists its bots, so the refusal is about the addressed
        // resource and not about the caller.
        let (status, described) = emulator
            .call(
                "GET",
                &format!("/app-instances/{}", uri_encode(&instance)),
                None,
            )
            .await;
        assert_eq!(
            (status, described["AppInstance"]["AppInstanceArn"].as_str()),
            (200, Some(instance.as_str())),
            "case {label:?}: expected the parent AppInstance to stay readable, got \
             {status} {described:?}"
        );
        assert_eq!(
            listed_bot(&emulator, &instance, arn).await,
            Value::Null,
            "case {label:?}: expected the listing to hold no row for {arn:?}"
        );
        assert_ne!(
            listed_bot(&emulator, &instance, &sibling).await,
            Value::Null,
            "case {label:?}: expected the listing to still report the sibling bot \
             {sibling:?}"
        );
    }

    // And the same read is refused identically once the parent instance is deleted under
    // it, so the two ways a bot ARN becomes unresolvable are one answer.
    let first_refusal = describe_bot(&emulator, &unresolvable[0].1).await;
    delete_app_instance(&emulator, &instance).await;
    for (label, arn) in &unresolvable {
        let refusal = describe_bot(&emulator, arn).await;
        assert_eq!(
            refusal, first_refusal,
            "case {label:?}: expected the refusal under a deleted parent to match the one \
             under a live parent ({first_refusal:?}), got {refusal:?}"
        );
    }
    let refusal = describe_bot(&emulator, &sibling).await;
    assert_eq!(
        refusal, first_refusal,
        "expected the cascaded sibling bot to earn the same refusal \
         ({first_refusal:?}), got {refusal:?}"
    );
}

/// The response header a REST+JSON client resolves a failure's error SHAPE from.
///
/// The shape name is what an SDK reports as the error class, in this header and in the
/// body's `__type`; the body's `Code` member is this service's own narrower enum
/// (`smithy_query(query="shape",shape="ErrorCode")`: `Forbidden`, `NotFound`, …), so the
/// class a caller branches on cannot be asserted from `Code` alone.
const ERROR_SHAPE_HEADER: &str = "x-amzn-errortype";

/// The shape name `ForbiddenException` carries on the wire.
const FORBIDDEN_SHAPE: &str = "ForbiddenException";

/// The shape name a report of absence would carry — the class this read must never serve.
const NOT_FOUND_SHAPE: &str = "NotFoundException";

/// `GET /app-instance-bots/{AppInstanceBotArn}`, with the error shape the wire named.
async fn describe_bot_with_shape(
    emulator: &Emulator,
    app_instance_bot_arn: &str,
) -> (u16, Option<String>, Value) {
    let (status, headers, body) = emulator
        .call_with_headers(
            "GET",
            &format!("/app-instance-bots/{}", uri_encode(app_instance_bot_arn)),
            None,
        )
        .await;
    let shape = headers.get(ERROR_SHAPE_HEADER).map(|value| {
        value
            .to_str()
            .expect("the error-shape header is a shape name")
            .to_string()
    });
    (status, shape, body)
}

/// RULE: a `DescribeAppInstanceBot` addressed to an `AppInstanceBotArn` this service
/// resolves no bot for is REFUSED as a request the caller may not make, never reported as
/// an absence — the class an SDK reads off the wire is the modelled `ForbiddenException`
/// (403, that shape in `x-amzn-errortype` and in the body's `__type`, `Code` the
/// `Forbidden` enum value, and the fixed forbidden-request sentence), never the
/// `NotFoundException` class, and nothing in the answer repeats the addressed ARN — so a
/// caller branching on the error class takes the refusal branch and learns nothing about
/// which bot ARNs this service holds.
///
/// The rows span every way a well-formed `ChimeArn` reaches this read without resolving to
/// a bot (a bot id the live parent never minted, at several shapes and sizes; a bot deleted
/// from it; a bot cascaded away with its parent; the same ARN in another region or
/// account; an ARN naming another resource kind), and the live bot beside them is the
/// control that the read still serves what it holds.
#[tokio::test]
async fn the_bot_read_refusal_names_the_forbidden_shape_an_sdk_reads() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("wire-class", "tok-wire-class").await;
    let user = emulator
        .user(&instance, "wire-class-user", "tok-wire-class-user")
        .await;
    let live = create_bot(
        &emulator,
        &instance,
        "tok-wire-class-live",
        Some("live"),
        Some("held"),
        &responds_to_configuration("BOTIDLIVEA", "en_US"),
    )
    .await;
    let deleted = create_bot(
        &emulator,
        &instance,
        "tok-wire-class-deleted",
        Some("deleted"),
        None,
        &invoked_by_configuration("BOTIDDELEB", "de_DE", "ALL", "NONE", None),
    )
    .await;
    delete_bot(&emulator, &deleted).await;

    let doomed = emulator
        .app_instance("wire-class-doomed", "tok-wire-class-doomed")
        .await;
    let cascaded = create_bot(
        &emulator,
        &doomed,
        "tok-wire-class-cascaded",
        None,
        None,
        &responds_to_configuration("BOTIDCASCB", "ja_JP"),
    )
    .await;
    delete_app_instance(&emulator, &doomed).await;

    // Bot ids of several shapes and sizes under the LIVE parent, so the refusal is pinned
    // over the class of unresolvable ARNs rather than over one id. Each ARN stays inside
    // the modelled bounds of the input (`smithy_query(query="shape",shape="ChimeArn")`:
    // len 5..1600, pattern `^arn:...:[^/].{0,1023}$`), so what is under test is the
    // refusal and not input validation.
    let long_id = "9".repeat(128);
    let unresolvable: Vec<(&str, String)> = vec![
        (
            "a uuid-shaped bot id the parent never minted",
            format!("{instance}/bot/7c4a1f20-9d3b-4e55-8a01-5c2d9b7e4f30"),
        ),
        (
            "a single-character bot id",
            format!("{instance}/bot/z"),
        ),
        ("a long numeric bot id", format!("{instance}/bot/{long_id}")),
        (
            "a bot id carrying non-ASCII characters",
            format!("{instance}/bot/ロボット"),
        ),
        ("a bot deleted from the live parent", deleted.clone()),
        ("a bot cascaded away with its parent", cascaded.clone()),
        (
            "the live bot's ARN in another region",
            in_another_region(&live),
        ),
        (
            "the live bot's ARN in another account",
            in_another_account(&live),
        ),
        ("an AppInstanceUser ARN", user.clone()),
    ];

    for (label, arn) in &unresolvable {
        let (status, shape, body) = describe_bot_with_shape(&emulator, arn).await;
        assert_eq!(
            (status, shape.as_deref()),
            (403, Some(FORBIDDEN_SHAPE)),
            "case {label:?}: expected {arn:?} to be refused with the {FORBIDDEN_SHAPE} \
             class, got {status} {shape:?} {body:?}"
        );
        assert_eq!(
            (
                body["__type"].as_str(),
                body["Code"].as_str(),
                body["Message"].as_str()
            ),
            (
                Some(FORBIDDEN_SHAPE),
                Some("Forbidden"),
                Some(FORBIDDEN_MESSAGE)
            ),
            "case {label:?}: expected the refusal body to name the forbidden shape and \
             carry the fixed sentence, got {body:?}"
        );
        let rendered = body.to_string();
        assert!(
            !rendered.contains(NOT_FOUND_SHAPE) && !rendered.contains("NotFound"),
            "case {label:?}: expected no absence class anywhere in the answer, got \
             {rendered}"
        );
        assert!(
            !rendered.contains(arn.as_str()),
            "case {label:?}: expected the answer to repeat the addressed ARN nowhere, got \
             {rendered}"
        );
        assert!(
            body.get("AppInstanceBot").is_none(),
            "case {label:?}: expected the refusal to report no bot, got {body:?}"
        );

        // The controls that make this a per-resource refusal: the same caller still reads
        // the parent instance and still lists its bots, which hold no row for the ARN just
        // refused.
        let (status, described) = emulator
            .call(
                "GET",
                &format!("/app-instances/{}", uri_encode(&instance)),
                None,
            )
            .await;
        assert_eq!(
            (status, described["AppInstance"]["AppInstanceArn"].as_str()),
            (200, Some(instance.as_str())),
            "case {label:?}: expected the parent AppInstance to stay readable, got \
             {status} {described:?}"
        );
        assert_eq!(
            listed_bot(&emulator, &instance, arn).await,
            Value::Null,
            "case {label:?}: expected the listing to hold no row for {arn:?}"
        );
    }

    // The control: the bot this service DOES hold is still reported, with no error shape
    // on the wire at all, so the refusal above is the answer for an unresolvable ARN and
    // not for every read.
    let (status, shape, body) = describe_bot_with_shape(&emulator, &live).await;
    assert_eq!(
        (status, shape.as_deref(), member(&body, "Name").as_str()),
        (200, None, Some("live")),
        "expected the live bot to be reported with no error shape, got {status} {shape:?} \
         {body:?}"
    );
}
