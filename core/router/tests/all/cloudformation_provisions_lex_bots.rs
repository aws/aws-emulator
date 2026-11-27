// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares the canonical ALIASED LEX BOT estate deploys: the bot,
//! the locale and intents inside it, the immutable VERSION cut from that locale, and the ALIAS
//! that points at the version — each created in the Amazon Lex V2 model-building plane that
//! owns it, and each published under the identity that service assigned.
//!
//! The rule under test: **a resource type's `Fn::GetAtt` surface is the set of return values
//! AWS publishes for that type, and the "does not exist in schema for" attribute error is
//! the answer only for an attribute OUTSIDE that set** — so `Fn::GetAtt` on an
//! `AWS::Lex::BotVersion`'s `BotVersion` resolves to the numeric version Amazon Lex assigned
//! when it cut the version, which is exactly what the `botVersion` of the alias beside it
//! takes, and the stack reaches `CREATE_COMPLETE`. Two crates have to agree for that to hold
//! (CloudFormation submits the template and makes the calls, the Lex V2 model-building plane
//! serves them, and IAM serves the role the bot runs as), so the suite lives here.
//!
//! The rows differ along what the rule generalises over: the template spelling (JSON with
//! long-form intrinsics, YAML with the short-form tags), how many locales the bot carries and
//! how many of them the version is cut over, whether the locales are built as they are
//! created, whether the alias carries a description and per-locale settings, and where the
//! resolved version is read — a resource PROPERTY of the alias and an `Outputs` entry.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode, opt_element, outputs_of};

/// The region every request here is signed for, and so the region the stack and its bot
/// belong to.
const REGION: &str = "us-east-1";

/// How long the suite is willing to wait for a stack operation or a bot locale's build to
/// settle, and how often it looks again. Generous: each is a transition the owning service's
/// own clock owes, and the suite waits for the CONDITION rather than for a fixed time.
const SETTLE_DEADLINE: Duration = Duration::from_secs(90);
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(100);

/// The signing name the Amazon Lex V2 model-building plane publishes (`sigv4Name: lex`).
const LEX: &str = "lex";

/// The resource types under test, spelled as the "AWS resource and property types reference"
/// spells them.
const BOT: &str = "AWS::Lex::Bot";
const BOT_VERSION: &str = "AWS::Lex::BotVersion";
const BOT_ALIAS: &str = "AWS::Lex::BotAlias";

/// The attribute of an `AWS::Lex::BotVersion` the canonical aliased-bot template reads, and
/// one that type does NOT publish — the negative control.
const VERSION_ATTRIBUTE: &str = "BotVersion";
const UNPUBLISHED_ATTRIBUTE: &str = "Status";

/// The `BotLocaleStatus` a locale an `AutoBuildBotLocales` deployment built reaches ("the bot
/// is ready for use and can be tested using any utterance"), and the one a locale that was
/// "saved but not built" rests in.
const BUILT: &str = "Built";
const NOT_BUILT: &str = "NotBuilt";

/// The working copy every locale, intent and slot is defined in, and the version a snapshot is
/// taken from: the modelled `DraftBotVersion` is `^DRAFT$`.
const DRAFT: &str = "DRAFT";

/// The emulator with the services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation action over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one operation of the Lex V2 model-building plane over its restJson1 wire.
async fn lex(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("models-v2-lex.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization(LEX))
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The `DescribeStacks` answer once the stack rests in a status that is not in progress — or
/// once it is GONE, which a delete leaves it — or the last one read when the deadline passes.
async fn settled_stack(router: &MultiServiceRouter, stack_name: &str) -> (u16, String) {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
        let settled = status != 200
            || opt_element(&body, "StackStatus")
                .is_some_and(|stack_status| !stack_status.ends_with("_IN_PROGRESS"));
        if settled || Instant::now() >= deadline {
            return (status, body);
        }
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// Create the stack and answer its settled `DescribeStacks` body.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", template),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    let (status, body) = settled_stack(router, stack_name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// Every reason the stack's own events carry for a resource that did not come up, which is
/// what a failed deployment has to be diagnosed from.
async fn failure_reasons(router: &MultiServiceRouter, stack_name: &str) -> Vec<String> {
    let (_, events) = cloudformation(
        router,
        "DescribeStackEvents",
        &[("StackName", stack_name)],
    )
    .await;
    elements(&events, "ResourceStatusReason")
}

/// The role a Lex bot runs as, declared in every template here so the bot's `RoleArn` is read
/// off a role the SAME stack creates — which is how the canonical template states it.
fn role_declaration() -> Value {
    json!({
        "Type": "AWS::IAM::Role",
        "Properties": {
            "AssumeRolePolicyDocument": {
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "lexv2.amazonaws.com" },
                    "Action": "sts:AssumeRole",
                }],
            },
        },
    })
}

/// One locale of a declared bot, carrying one intent with a sample utterance — which is what a
/// build of the locale needs to succeed — and, for the first locale, a custom slot type and a
/// slot of that type ordered by the intent's `SlotPriorities`.
fn locale_declaration(locale_id: &str, with_a_slot: bool) -> Value {
    let mut intent = json!({
        "Name": format!("Order{}", locale_id.replace('_', "")),
        "Description": "the intent the stack declares",
        "SampleUtterances": [
            { "Utterance": "I want to order a pizza" },
            { "Utterance": "order a pizza" },
        ],
    });
    let mut locale = json!({
        "LocaleId": locale_id,
        "NluConfidenceThreshold": 0.4,
        "Description": format!("the {locale_id} locale"),
    });
    if with_a_slot {
        intent["Slots"] = json!([{
            "Name": "Size",
            "SlotTypeName": "PizzaSize",
            "ValueElicitationSetting": {
                "SlotConstraint": "Required",
                "PromptSpecification": {
                    "MaxRetries": 2,
                    "MessageGroupsList": [{
                        "Message": { "PlainTextMessage": { "Value": "What size?" } },
                    }],
                },
            },
        }]);
        intent["SlotPriorities"] = json!([{ "Priority": 1, "SlotName": "Size" }]);
        locale["SlotTypes"] = json!([{
            "Name": "PizzaSize",
            "Description": "the sizes a pizza comes in",
            "SlotTypeValues": [
                { "SampleValue": { "Value": "large" } },
                { "SampleValue": { "Value": "small" } },
            ],
            "ValueSelectionSetting": { "ResolutionStrategy": "OriginalValue" },
        }]);
    }
    locale["Intents"] = json!([intent]);
    locale
}

/// What one row expects of the estate its stack declares.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    /// The locales the bot carries, in the order the declaration states them.
    locales: Vec<&'static str>,
    /// Whether the declaration asked for every locale to be BUILT as it was created.
    auto_built: bool,
    /// The alias name the declaration states.
    alias_name: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        // The canonical aliased-bot estate: one locale, built as it is created, a version cut
        // from it, and an alias whose `BotVersion` property is an `Fn::GetAtt` of that version
        // — the wiring the whole rule is about. Every published value is read back through the
        // stack's outputs as well.
        Row {
            case: "one built locale, the alias's version read through Fn::GetAtt",
            stack_name: "lex-aliased-bot",
            template: json!({
                "Resources": {
                    "BotRole": role_declaration(),
                    "Bot": {
                        "Type": BOT,
                        "Properties": {
                            "Name": "BookTripBotWithCFN",
                            "RoleArn": { "Fn::GetAtt": ["BotRole", "Arn"] },
                            "DataPrivacy": { "ChildDirected": false },
                            "IdleSessionTTLInSeconds": 300,
                            "AutoBuildBotLocales": true,
                            "BotLocales": [locale_declaration("en_US", true)],
                            "BotTags": [{ "Key": "team", "Value": "voice" }],
                        },
                    },
                    "BookTripBotVersionWithCFN": {
                        "Type": BOT_VERSION,
                        "Properties": {
                            "BotId": { "Ref": "Bot" },
                            "BotVersionLocaleSpecification": [{
                                "LocaleId": "en_US",
                                "BotVersionLocaleDetails": { "SourceBotVersion": DRAFT },
                            }],
                        },
                    },
                    "FirstBotAliasWithCFN": {
                        "Type": BOT_ALIAS,
                        "Properties": {
                            "BotId": { "Ref": "Bot" },
                            "BotAliasName": "FirstBotAlias",
                            "BotVersion": {
                                "Fn::GetAtt": ["BookTripBotVersionWithCFN", VERSION_ATTRIBUTE],
                            },
                        },
                    },
                },
                "Outputs": {
                    "BotRef": { "Value": { "Ref": "Bot" } },
                    "BotId": { "Value": { "Fn::GetAtt": ["Bot", "Id"] } },
                    "BotArn": { "Value": { "Fn::GetAtt": ["Bot", "Arn"] } },
                    "Version": {
                        "Value": {
                            "Fn::GetAtt": ["BookTripBotVersionWithCFN", VERSION_ATTRIBUTE],
                        },
                    },
                    "AliasId": {
                        "Value": { "Fn::GetAtt": ["FirstBotAliasWithCFN", "BotAliasId"] },
                    },
                    "AliasArn": { "Value": { "Fn::GetAtt": ["FirstBotAliasWithCFN", "Arn"] } },
                },
            })
            .to_string(),
            locales: vec!["en_US"],
            auto_built: true,
            alias_name: "FirstBotAlias",
        },
        // The YAML spelling with the short-form tags: two locales, neither built as it is
        // created, the version cut over both of them, and an alias carrying a description and
        // per-locale settings beside the version it reads the same way.
        Row {
            case: "two unbuilt locales, an alias carrying locale settings, YAML spelling",
            stack_name: "lex-two-locale-bot",
            template: format!(
                "\
Resources:
  BotRole:
    Type: AWS::IAM::Role
    Properties:
      AssumeRolePolicyDocument:
        Version: '2012-10-17'
        Statement:
          - Effect: Allow
            Principal:
              Service: lexv2.amazonaws.com
            Action: sts:AssumeRole
  Bot:
    Type: {BOT}
    Properties:
      Name: TwoLocaleBotWithCFN
      RoleArn: !GetAtt BotRole.Arn
      DataPrivacy:
        ChildDirected: false
      IdleSessionTTLInSeconds: 600
      BotLocales: {locales}
  Version:
    Type: {BOT_VERSION}
    Properties:
      BotId: !Ref Bot
      Description: the version both locales were cut into
      BotVersionLocaleSpecification: {specification}
  Alias:
    Type: {BOT_ALIAS}
    Properties:
      BotId: !Ref Bot
      BotAliasName: Production
      Description: the alias the stack publishes
      BotVersion: !GetAtt Version.{VERSION_ATTRIBUTE}
      BotAliasLocaleSettings:
        - LocaleId: en_US
          BotAliasLocaleSetting:
            Enabled: true
Outputs:
  BotRef:
    Value: !Ref Bot
  BotId:
    Value: !GetAtt Bot.Id
  Version:
    Value: !GetAtt Version.{VERSION_ATTRIBUTE}
  AliasId:
    Value: !GetAtt Alias.BotAliasId
",
                locales = serde_json::to_string(&json!([
                    locale_declaration("en_US", false),
                    locale_declaration("de_DE", false),
                ]))
                .expect("the locales serialize"),
                specification = serde_json::to_string(&json!([
                    { "LocaleId": "en_US", "BotVersionLocaleDetails": { "SourceBotVersion": DRAFT } },
                    { "LocaleId": "de_DE", "BotVersionLocaleDetails": { "SourceBotVersion": DRAFT } },
                ]))
                .expect("the specification serializes"),
            ),
            locales: vec!["en_US", "de_DE"],
            auto_built: false,
            alias_name: "Production",
        },
    ]
}

/// One output of the settled stack, by key.
fn output(outputs: &BTreeMap<String, String>, case: &str, key: &str, body: &str) -> String {
    outputs
        .get(key)
        .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"))
        .clone()
}

/// Whether `id` has the form Amazon Lex mints an `Id` in: the modelled `Id` and `BotAliasId`
/// are both `[0-9a-zA-Z]{10}`.
fn is_a_minted_lex_id(id: &str) -> bool {
    id.len() == 10 && id.chars().all(|character| character.is_ascii_alphanumeric())
}

/// Whether `version` has the form of a `NumericalBotVersion`: `^[0-9]+$`, length 1 to 5.
fn is_a_numeric_bot_version(version: &str) -> bool {
    (1..=5).contains(&version.len()) && version.chars().all(|digit| digit.is_ascii_digit())
}

/// The locale the deadline waits for, read again until its status is one a build has settled
/// on — or the generous deadline passes. Waits for the CONDITION, not for a fixed time.
async fn settled_locale(
    router: &MultiServiceRouter,
    bot_id: &str,
    locale_id: &str,
) -> (u16, Value) {
    let path = format!("/bots/{bot_id}/botversions/{DRAFT}/botlocales/{locale_id}");
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, locale) = lex(router, "GET", &path, None).await;
        let status_text = locale
            .get("botLocaleStatus")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if status != 200
            || matches!(status_text.as_str(), BUILT | NOT_BUILT | "Failed")
            || Instant::now() >= deadline
        {
            return (status, locale);
        }
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

#[tokio::test]
async fn a_deployed_stack_publishes_the_version_amazon_lex_assigned_and_the_alias_on_it() {
    for row in rows() {
        let router = build_router();
        let case = row.case;
        let body = create_and_settle(&router, case, row.stack_name, &row.template).await;
        assert!(
            !body.contains("does not exist in schema for"),
            "case {case:?}: expected Fn::GetAtt on a published return value to resolve, got \
             {body}"
        );
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the create to complete, got {:?} with reasons {:?}",
            opt_element(&body, "StackStatusReason"),
            failure_reasons(&router, row.stack_name).await
        );
        let outputs = outputs_of(&body);

        // "Ref returns the bot Id": the identity Amazon Lex minted, which every bot-addressed
        // call takes — and the bot that id names is the one the template declared.
        let bot_id = output(&outputs, case, "BotId", &body);
        assert!(
            is_a_minted_lex_id(&bot_id),
            "case {case:?}: expected the published Id to have the minted \
             ^[0-9a-zA-Z]{{10}}$ form, got {bot_id:?}"
        );
        assert_eq!(
            output(&outputs, case, "BotRef", &body),
            bot_id,
            "case {case:?}: expected Ref to publish the bot id, got {body}"
        );
        let (status, bot) = lex(&router, "GET", &format!("/bots/{bot_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the model-building plane to resolve the published bot id \
             {bot_id:?}, got {status} with {bot}"
        );
        let role = bot
            .get("roleArn")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            role.contains(":role/"),
            "case {case:?}: expected the bot to run as the role the stack created, got {bot}"
        );

        // Every locale the declaration states was created inside the bot, and each carries the
        // status the declaration earned it: a BUILT locale is one whose intents and their
        // sample utterances really reached Amazon Lex, a build having nothing to build without
        // them.
        for locale_id in &row.locales {
            let (status, locale) = settled_locale(&router, &bot_id, locale_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the {locale_id} locale to exist, got {status} with \
                 {locale}"
            );
            assert_eq!(
                locale.get("botLocaleStatus").and_then(Value::as_str),
                Some(if row.auto_built { BUILT } else { NOT_BUILT }),
                "case {case:?}: expected the {locale_id} locale's status, got {locale}"
            );
        }

        // The attribute the whole rule is about: `Fn::GetAtt` on the version resource resolves
        // to the number Amazon Lex assigned, which is a `NumericalBotVersion` and is the
        // version that service holds for the bot.
        let version = output(&outputs, case, "Version", &body);
        assert!(
            is_a_numeric_bot_version(&version),
            "case {case:?}: expected the published BotVersion to be a NumericalBotVersion, got \
             {version:?}"
        );
        let (status, reported) = lex(
            &router,
            "GET",
            &format!("/bots/{bot_id}/botversions/{version}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Amazon Lex to hold the published version {version:?}, got \
             {status} with {reported}"
        );
        assert_eq!(
            reported.get("botVersion").and_then(Value::as_str),
            Some(version.as_str()),
            "case {case:?}: expected the version to report the number the stack published, got \
             {reported}"
        );

        // And the alias was created with that resolved version as its `botVersion`, which is
        // what the alias's property read off the version resource carries.
        let alias_id = output(&outputs, case, "AliasId", &body);
        assert!(
            is_a_minted_lex_id(&alias_id),
            "case {case:?}: expected a minted botAliasId, got {alias_id:?}"
        );
        let (status, alias) = lex(
            &router,
            "GET",
            &format!("/bots/{bot_id}/botaliases/{alias_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Amazon Lex to resolve the published alias {alias_id:?}, \
             got {status} with {alias}"
        );
        assert_eq!(
            (
                alias.get("botVersion").and_then(Value::as_str),
                alias.get("botAliasName").and_then(Value::as_str),
                alias.get("botId").and_then(Value::as_str),
            ),
            (
                Some(version.as_str()),
                Some(row.alias_name),
                Some(bot_id.as_str()),
            ),
            "case {case:?}: expected the alias to point at the version the stack published, got \
             {alias}"
        );
        if let Some(published) = outputs.get("AliasArn") {
            assert_eq!(
                published,
                &format!("arn:aws:lex:{REGION}:000000000000:bot-alias/{bot_id}/{alias_id}"),
                "case {case:?}: expected the alias ARN around the minted ids, got {body}"
            );
        }
    }
}

/// The same estate deployed the way `cdk deploy` deploys one — `CreateChangeSet` followed by
/// `ExecuteChangeSet` rather than `CreateStack` — resolves the version attribute identically:
/// which operation submitted the template is no part of what `Fn::GetAtt` resolves.
#[tokio::test]
async fn the_change_set_path_resolves_the_version_attribute_too() {
    let router = build_router();
    let case = "the canonical estate deployed through a change set";
    let stack_name = "lex-change-set-bot";
    let change_set = "deploy-the-lex-estate";
    let row = rows().remove(0);

    let (status, created) = cloudformation(
        &router,
        "CreateChangeSet",
        &[
            ("StackName", stack_name),
            ("ChangeSetName", change_set),
            ("ChangeSetType", "CREATE"),
            ("TemplateBody", &row.template),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateChangeSet, got {status} with {created}"
    );

    // The change set is ready once it rests in CREATE_COMPLETE, which is the status
    // `CreateChangeSet` answers `CREATE_PENDING` and settles to — the condition a client waits
    // for before executing one. A terminal FAILED ends the wait too.
    let deadline = Instant::now() + SETTLE_DEADLINE;
    let described = loop {
        let (status, body) = cloudformation(
            &router,
            "DescribeChangeSet",
            &[("StackName", stack_name), ("ChangeSetName", change_set)],
        )
        .await;
        let settled = status != 200
            || opt_element(&body, "Status").is_some_and(|change_set_status| {
                matches!(change_set_status.as_str(), "CREATE_COMPLETE" | "FAILED")
            });
        if settled || Instant::now() >= deadline {
            break body;
        }
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    };
    assert_eq!(
        opt_element(&described, "Status").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the change set to be ready to execute, got {described}"
    );

    let (status, executed) = cloudformation(
        &router,
        "ExecuteChangeSet",
        &[("StackName", stack_name), ("ChangeSetName", change_set)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ExecuteChangeSet, got {status} with {executed}"
    );
    let (status, body) = settled_stack(&router, stack_name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with {body}"
    );
    assert!(
        !body.contains("does not exist in schema for"),
        "case {case:?}: expected Fn::GetAtt on a published return value to resolve, got {body}"
    );
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the executed change set to complete, got {:?} with reasons {:?}",
        opt_element(&body, "StackStatusReason"),
        failure_reasons(&router, stack_name).await
    );

    // And the version the stack published is the one Amazon Lex holds for the bot.
    let outputs = outputs_of(&body);
    let bot_id = output(&outputs, case, "BotId", &body);
    let version = output(&outputs, case, "Version", &body);
    assert!(
        is_a_numeric_bot_version(&version),
        "case {case:?}: expected a NumericalBotVersion, got {version:?}"
    );
    let (status, reported) = lex(
        &router,
        "GET",
        &format!("/bots/{bot_id}/botversions/{version}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected Amazon Lex to hold the published version {version:?}, got \
         {status} with {reported}"
    );
}

/// The negative control: `Fn::GetAtt` on an attribute the type genuinely does NOT publish is
/// still refused with AWS's own schema error, naming the attribute requested and the TYPE of
/// the resource whose schema has no such member — the refusal the published `BotVersion` must
/// never get.
#[tokio::test]
async fn an_attribute_the_type_does_not_publish_is_still_refused_as_a_template_error() {
    let router = build_router();
    let case = "an unpublished attribute of a bot version";
    let stack_name = "lex-unpublished-attribute";
    let template = json!({
        "Resources": {
            "BotRole": role_declaration(),
            "Bot": {
                "Type": BOT,
                "Properties": {
                    "Name": "UnpublishedAttributeBot",
                    "RoleArn": { "Fn::GetAtt": ["BotRole", "Arn"] },
                    "DataPrivacy": { "ChildDirected": false },
                    "IdleSessionTTLInSeconds": 300,
                    "BotLocales": [locale_declaration("en_US", false)],
                },
            },
            "BookTripBotVersionWithCFN": {
                "Type": BOT_VERSION,
                "Properties": {
                    "BotId": { "Ref": "Bot" },
                    "BotVersionLocaleSpecification": [{
                        "LocaleId": "en_US",
                        "BotVersionLocaleDetails": { "SourceBotVersion": DRAFT },
                    }],
                },
            },
            "FirstBotAliasWithCFN": {
                "Type": BOT_ALIAS,
                "Properties": {
                    "BotId": { "Ref": "Bot" },
                    "BotAliasName": "FirstBotAlias",
                    "BotVersion": {
                        "Fn::GetAtt": ["BookTripBotVersionWithCFN", VERSION_ATTRIBUTE],
                    },
                    "Description": {
                        "Fn::GetAtt": ["BookTripBotVersionWithCFN", UNPUBLISHED_ATTRIBUTE],
                    },
                },
            },
        },
    })
    .to_string();

    let body = create_and_settle(&router, case, stack_name, &template).await;
    let stack_status = opt_element(&body, "StackStatus").unwrap_or_default();
    assert!(
        stack_status != "CREATE_COMPLETE",
        "case {case:?}: expected the stack to fail on an attribute the type does not publish, \
         got {body}"
    );

    // The reason the failing resource carries is AWS's own fixed text around the two names the
    // error is about: the attribute requested and the type whose schema does not declare it.
    let (status, events) = cloudformation(
        &router,
        "DescribeStackEvents",
        &[("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackEvents, got {status} with {events}"
    );
    let expected = format!(
        "Requested attribute {UNPUBLISHED_ATTRIBUTE} does not exist in schema for {BOT_VERSION}"
    );
    assert!(
        events.contains(&expected),
        "case {case:?}: expected the failing resource to report {expected:?}, got {events}"
    );
}

/// Deleting the stack deletes the estate it created: the alias, the version and the bot all
/// stop resolving in the owning service, which is the other half of the rule.
#[tokio::test]
async fn deleting_the_stack_deletes_the_bot_estate_it_created() {
    let router = build_router();
    let case = "a Lex estate deleted with its stack";
    let stack_name = "lex-deleted-bot";
    let row = rows().remove(0);
    let body = create_and_settle(&router, case, stack_name, &row.template).await;
    let outputs = outputs_of(&body);
    let bot_id = output(&outputs, case, "BotId", &body);
    let version = output(&outputs, case, "Version", &body);
    let alias_id = output(&outputs, case, "AliasId", &body);

    let (status, deleted) =
        cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {deleted}"
    );
    let (status, settled) = settled_stack(&router, stack_name).await;
    assert!(
        status != 200
            || opt_element(&settled, "StackStatus")
                .is_none_or(|stack_status| stack_status == "DELETE_COMPLETE"),
        "case {case:?}: expected the delete to complete, got {status} with {settled}"
    );

    for (path, what) in [
        (format!("/bots/{bot_id}/botaliases/{alias_id}"), "alias"),
        (format!("/bots/{bot_id}/botversions/{version}"), "version"),
        (format!("/bots/{bot_id}"), "bot"),
    ] {
        let (status, gone) = lex(&router, "GET", &path, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the {what} to be gone with its stack, got {status} with \
             {gone}"
        );
    }
}
