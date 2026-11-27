// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::Lex::Bot` whose properties carry the enum
//! values the resource type DOCUMENTS deploys, and the resources it declares inside the bot
//! carry those values in the Amazon Lex V2 model-building plane.
//!
//! The rule under test: **a resource provider sends an enum-valued property in the spelling
//! the enum of the API it calls admits, whatever spelling the resource type documents for that
//! property** — so a slot type declaring CloudFormation's documented
//! `ValueSelectionSetting.ResolutionStrategy` (`ORIGINAL_VALUE | TOP_RESOLUTION |
//! CONCATENATION`) is created with the `SlotValueResolutionStrategy` the API spells
//! (`OriginalValue | TopResolution | Concatenation`), and the stack reaches `CREATE_COMPLETE`.
//! Two crates have to agree for that to hold (CloudFormation submits the template and makes
//! the calls, the Lex V2 model-building plane serves them, and IAM serves the role the bot runs
//! as), so the suite lives here.
//!
//! The rows differ along what the rule generalises over: which value of the enum is declared,
//! which spelling it is declared in (CloudFormation's own and the API's, which for this
//! property differ and for its siblings coincide), which enum-valued property carries it
//! (a slot type's resolution strategy, a slot's constraint and obfuscation, a prompt's message
//! selection strategy, a locale's voice engine) and how deeply nested in the declaration it
//! sits. A value NO enum admits is the negative control: it is still refused, with the
//! constraint violation the API answers.

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

/// How long the suite is willing to wait for a stack operation to settle, and how often it
/// looks again. Generous: each is a transition the owning service's own clock owes, and the
/// suite waits for the CONDITION rather than for a fixed time.
const SETTLE_DEADLINE: Duration = Duration::from_secs(90);
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(100);

/// The signing name the Amazon Lex V2 model-building plane publishes (`sigv4Name: lex`).
const LEX: &str = "lex";

/// The resource type under test, spelled as the "AWS resource and property types reference"
/// spells it.
const BOT: &str = "AWS::Lex::Bot";

/// The working copy every locale, slot type, intent and slot is defined in: the modelled
/// `DraftBotVersion` is `^DRAFT$`.
const DRAFT: &str = "DRAFT";

/// The locale every bot here declares.
const LOCALE: &str = "en_US";

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

/// The `DescribeStacks` answer once the stack rests in a status that is not in progress, or the
/// last one read when the generous deadline passes.
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
    let (_, events) =
        cloudformation(router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
    elements(&events, "ResourceStatusReason")
}

/// The role a Lex bot runs as, declared in every template here so the bot's `RoleArn` is read
/// off a role the SAME stack creates.
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

/// A whole template around one locale declaration, with the bot's id published as an output.
fn template_around(bot_name: &str, locale: Value) -> String {
    json!({
        "Resources": {
            "BotRole": role_declaration(),
            "Bot": {
                "Type": BOT,
                "Properties": {
                    "Name": bot_name,
                    "RoleArn": { "Fn::GetAtt": ["BotRole", "Arn"] },
                    "DataPrivacy": { "ChildDirected": false },
                    "IdleSessionTTLInSeconds": 300,
                    "BotLocales": [locale],
                },
            },
        },
        "Outputs": { "BotId": { "Value": { "Ref": "Bot" } } },
    })
    .to_string()
}

/// One output of the settled stack, by key.
fn output(outputs: &BTreeMap<String, String>, case: &str, key: &str, body: &str) -> String {
    outputs
        .get(key)
        .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"))
        .clone()
}

/// The URI every call on a resource inside the draft locale of a bot is scoped by.
fn locale_scope(bot_id: &str) -> String {
    format!("/bots/{bot_id}/botversions/{DRAFT}/botlocales/{LOCALE}")
}

/// The ids the locale's slot types are held under, by the name each was declared with.
async fn slot_type_ids(
    router: &MultiServiceRouter,
    case: &str,
    bot_id: &str,
) -> BTreeMap<String, String> {
    let (status, listed) = lex(
        router,
        "POST",
        &format!("{}/slottypes", locale_scope(bot_id)),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListSlotTypes, got {status} with {listed}"
    );
    summaries_by_name(&listed, "slotTypeSummaries", "slotTypeName", "slotTypeId")
}

/// The ids a list answer reports, keyed by the name each entry carries.
fn summaries_by_name(
    listed: &Value,
    summaries: &str,
    name_member: &str,
    id_member: &str,
) -> BTreeMap<String, String> {
    listed
        .get(summaries)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let name = entry.get(name_member).and_then(Value::as_str)?;
                    let id = entry.get(id_member).and_then(Value::as_str)?;
                    Some((name.to_string(), id.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One slot-type row: the value the declaration states for `ResolutionStrategy`, and the
/// `SlotValueResolutionStrategy` the API must end up holding for it.
struct StrategyRow {
    case: &'static str,
    slot_type_name: &'static str,
    declared: &'static str,
    expected: &'static str,
}

/// Every value of the enum, in the spelling CloudFormation's `AWS::Lex::Bot` documents for
/// `ValueSelectionSetting.ResolutionStrategy` (`ORIGINAL_VALUE | TOP_RESOLUTION |
/// CONCATENATION`), beside the spelling the API's own `SlotValueResolutionStrategy` admits
/// (`smithy_query(query="shape",shape="SlotValueResolutionStrategy")` on the lexmodelsv2
/// model) — which is the control row, the two spellings coinciding for no value of this enum.
fn strategy_rows() -> Vec<StrategyRow> {
    vec![
        StrategyRow {
            case: "the documented spelling of the default strategy",
            slot_type_name: "PizzaSize",
            declared: "ORIGINAL_VALUE",
            expected: "OriginalValue",
        },
        StrategyRow {
            case: "the documented spelling of the resolution-list strategy",
            slot_type_name: "PizzaCrust",
            declared: "TOP_RESOLUTION",
            expected: "TopResolution",
        },
        StrategyRow {
            case: "the documented spelling of the concatenating strategy",
            slot_type_name: "PizzaTopping",
            declared: "CONCATENATION",
            expected: "Concatenation",
        },
        StrategyRow {
            case: "the API's own spelling, which is passed through",
            slot_type_name: "PizzaSauce",
            declared: "OriginalValue",
            expected: "OriginalValue",
        },
    ]
}

/// One declared slot type, named and carrying the value-selection setting the row states.
fn slot_type_declaration(name: &str, resolution_strategy: &str) -> Value {
    json!({
        "Name": name,
        "Description": format!("the {name} slot type"),
        "SlotTypeValues": [
            { "SampleValue": { "Value": "large" } },
            { "SampleValue": { "Value": "small" } },
        ],
        "ValueSelectionSetting": { "ResolutionStrategy": resolution_strategy },
    })
}

#[tokio::test]
async fn a_documented_resolution_strategy_spelling_provisions_the_slot_type() {
    let router = build_router();
    let stack_name = "lex-resolution-strategy-spellings";
    let rows = strategy_rows();
    let locale = json!({
        "LocaleId": LOCALE,
        "NluConfidenceThreshold": 0.4,
        "SlotTypes": rows
            .iter()
            .map(|row| slot_type_declaration(row.slot_type_name, row.declared))
            .collect::<Vec<_>>(),
    });
    let body = create_and_settle(
        &router,
        "every documented ResolutionStrategy spelling",
        stack_name,
        &template_around("ResolutionStrategyBot", locale),
    )
    .await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {:?}: expected the create to complete, got {:?} with reasons {:?}",
        "every documented ResolutionStrategy spelling",
        opt_element(&body, "StackStatusReason"),
        failure_reasons(&router, stack_name).await
    );

    let outputs = outputs_of(&body);
    let bot_id = output(
        &outputs,
        "every documented ResolutionStrategy spelling",
        "BotId",
        &body,
    );
    let ids = slot_type_ids(
        &router,
        "every documented ResolutionStrategy spelling",
        &bot_id,
    )
    .await;

    for row in rows {
        let case = row.case;
        let slot_type_id = ids.get(row.slot_type_name).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the {} slot type to be held by Amazon Lex, got {ids:?}",
                row.slot_type_name
            )
        });
        let (status, described) = lex(
            &router,
            "GET",
            &format!("{}/slottypes/{slot_type_id}", locale_scope(&bot_id)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeSlotType, got {status} with \
             {described}"
        );
        let reported = described
            .get("valueSelectionSetting")
            .and_then(|setting| setting.get("resolutionStrategy"))
            .and_then(Value::as_str);
        assert_eq!(
            reported,
            Some(row.expected),
            "case {case:?}: expected the slot type declared with {:?} to hold {:?}, got {:?} \
             in {described}",
            row.declared,
            row.expected,
            reported
        );
    }
}

/// The sibling enum-valued properties of the same resource type, whose documented spelling IS
/// the API's: each reaches the call unchanged, nested at every depth the declaration puts one
/// at — the locale's voice engine, the advanced recognition setting inside a slot type's own
/// value-selection setting, and the constraint, obfuscation and message selection strategy of
/// a slot declared inside an intent.
#[tokio::test]
async fn the_sibling_enum_valued_properties_reach_the_api_in_its_own_spelling() {
    let router = build_router();
    let case = "the sibling enum-valued properties of a declared bot";
    let stack_name = "lex-sibling-enum-properties";
    let mut slot_type = slot_type_declaration("PizzaSize", "TOP_RESOLUTION");
    slot_type["ValueSelectionSetting"]["AdvancedRecognitionSetting"] =
        json!({ "AudioRecognitionStrategy": "UseSlotValuesAsCustomVocabulary" });
    let locale = json!({
        "LocaleId": LOCALE,
        "NluConfidenceThreshold": 0.4,
        "VoiceSettings": { "VoiceId": "Ivy", "Engine": "neural" },
        "SlotTypes": [slot_type],
        "Intents": [{
            "Name": "OrderPizza",
            "SampleUtterances": [{ "Utterance": "I want to order a pizza" }],
            "Slots": [{
                "Name": "Size",
                "SlotTypeName": "PizzaSize",
                "ObfuscationSetting": { "ObfuscationSettingType": "DefaultObfuscation" },
                "ValueElicitationSetting": {
                    "SlotConstraint": "Required",
                    "PromptSpecification": {
                        "MaxRetries": 2,
                        "MessageSelectionStrategy": "Ordered",
                        "MessageGroupsList": [{
                            "Message": { "PlainTextMessage": { "Value": "What size?" } },
                        }],
                    },
                },
            }],
            "SlotPriorities": [{ "Priority": 1, "SlotName": "Size" }],
        }],
    });
    let body = create_and_settle(
        &router,
        case,
        stack_name,
        &template_around("SiblingEnumBot", locale),
    )
    .await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the create to complete, got {:?} with reasons {:?}",
        opt_element(&body, "StackStatusReason"),
        failure_reasons(&router, stack_name).await
    );
    let outputs = outputs_of(&body);
    let bot_id = output(&outputs, case, "BotId", &body);

    // The locale's own enum-valued property: the Amazon Polly engine the voice settings name.
    let (status, described) = lex(&router, "GET", &locale_scope(&bot_id), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeBotLocale, got {status} with {described}"
    );
    let engine = described
        .get("voiceSettings")
        .and_then(|settings| settings.get("engine"))
        .and_then(Value::as_str);
    assert_eq!(
        engine,
        Some("neural"),
        "case {case:?}: expected the locale's declared voice engine, got {engine:?} in \
         {described}"
    );

    // The slot type's: the strategy it resolves values by, and the audio recognition strategy
    // of the advanced recognition setting nested inside the same property.
    let ids = slot_type_ids(&router, case, &bot_id).await;
    let slot_type_id = ids.get("PizzaSize").unwrap_or_else(|| {
        panic!("case {case:?}: expected the declared slot type, got {ids:?}")
    });
    let (status, described) = lex(
        &router,
        "GET",
        &format!("{}/slottypes/{slot_type_id}", locale_scope(&bot_id)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSlotType, got {status} with {described}"
    );
    let setting = described.get("valueSelectionSetting");
    let reported = (
        setting
            .and_then(|setting| setting.get("resolutionStrategy"))
            .and_then(Value::as_str),
        setting
            .and_then(|setting| setting.get("advancedRecognitionSetting"))
            .and_then(|advanced| advanced.get("audioRecognitionStrategy"))
            .and_then(Value::as_str),
    );
    assert_eq!(
        reported,
        (
            Some("TopResolution"),
            Some("UseSlotValuesAsCustomVocabulary")
        ),
        "case {case:?}: expected the slot type's declared enum values to reach Amazon Lex, got \
         {reported:?} in {described}"
    );

    // And the slot's own, which are proven by the create having completed at all: the API
    // models `SlotValueElicitationSetting.slotConstraint` as a @required enum member, so a
    // value its enum did not admit would have refused `CreateSlot` and failed the resource —
    // which is what the stack status asserted above rules out. (The slot itself is not read
    // back here: `ListIntents` is still a stub, so no client can resolve the intent id the
    // read takes.)
}

/// The negative control: a value NO spelling of the enum admits is still refused, so the
/// translation never invents an admissible value. The resource carries the constraint
/// violation the API answers, naming the member and the values its enum admits.
#[tokio::test]
async fn a_value_the_enum_does_not_admit_is_still_refused() {
    let router = build_router();
    let case = "a resolution strategy no spelling of the enum admits";
    let stack_name = "lex-unknown-resolution-strategy";
    let locale = json!({
        "LocaleId": LOCALE,
        "NluConfidenceThreshold": 0.4,
        "SlotTypes": [slot_type_declaration("PizzaSize", "MOST_LIKELY_VALUE")],
    });
    let body = create_and_settle(
        &router,
        case,
        stack_name,
        &template_around("UnknownStrategyBot", locale),
    )
    .await;
    let stack_status = opt_element(&body, "StackStatus").unwrap_or_default();
    assert!(
        stack_status != "CREATE_COMPLETE",
        "case {case:?}: expected the stack to fail on a value the enum does not admit, got \
         {body}"
    );
    let reasons = failure_reasons(&router, stack_name).await;
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains("resolutionStrategy")
                && reason.contains("Member must satisfy enum value set")),
        "case {case:?}: expected the refused resource to report the constraint violation, got \
         {reasons:?}"
    );
}
