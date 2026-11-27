// SPDX-License-Identifier: CC0-1.0

//! How a RESOURCE is linked to a channel configuration and unlinked again, driven over the
//! wire (restJson1) through the same `MultiServiceRouter` the production router serves
//! with.
//!
//! The rules every assertion here is about:
//!
//! * **The resource-scoped authorization decision comes FIRST, for any `Resource` naming
//!   no custom action of this account.** `Resource` identifies the resource the call
//!   addresses, so whether the caller's credentials are valid for that target is decided
//!   before the member's own `@length`/`@pattern` is judged: a value no custom action of
//!   this account could ever be minted under — one carrying a region segment, another
//!   account's id, an ARN of another service, type or partition, or a string that is not an
//!   ARN at all — is refused HTTP 403 `UnauthorizedException` with the fixed message
//!   `Unauthorized`, whether or not it satisfies that pattern.
//! * **Every OTHER member outside its modelled constraints is client input.** A
//!   `ChatConfiguration` outside its `@length`/`@pattern` names no resource this call is
//!   authorized against, so it keeps the request-boundary refusal: this service's
//!   client-input class `InvalidRequestException` (HTTP 400, the fixed message
//!   `Invalid request body`).
//! * **A target has to be able to host the link.** An associate against anything other
//!   than a channel configuration this account holds — an Amazon Chime webhook
//!   configuration, or an ARN no configuration is stored under — is refused with the
//!   modelled server error `InternalServiceError` (HTTP 500, the fixed message
//!   `Internal Server Error`) and leaves no association behind, however many callers ask
//!   at once.
//! * **A link round-trips, and both operations are idempotent.** A successful associate is
//!   reported by `ListAssociations`, repeating it leaves the configuration linked once,
//!   and a disassociate answers its empty 204 success whether or not a link was there to
//!   remove.
//!
//! Every table spans what the rule generalises over — the refused ARN's spelling, the
//! kind of target, the number of resources linked and the page size a listing asks for —
//! rather than one recorded request.

use std::sync::Arc;

use serde_json::json;
use service_com_amazonaws_chatbot_wheatleyorchestration_20171011 as chatbot;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{post, Answer, OTHER_REGION, REGION};

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`), spelled out
/// here so the test pins the ARN a client actually sees.
const ACCOUNT: &str = "000000000000";

/// The Slack workspace the channel configurations below are created in, with the display
/// name it is authorized under. A workspace has to be authorized with the account before a
/// channel can be configured in it, and no operation of this model performs that handshake
/// — so the harness registers it through the state seam, as the console handshake would.
const WORKSPACE: (&str, &str) = ("T0123ABCD", "Primary Workspace");

const IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-role";

/// The modelled 403 a `Resource` naming no custom action of this account is refused with,
/// and the fixed sentence it carries.
const UNAUTHORIZED_EXCEPTION: &str = "UnauthorizedException";
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// The modelled 400 a value outside a member's modelled constraints is refused with, and the
/// fixed sentence a boundary refusal carries (`service::INVALID_REQUEST_BODY_MESSAGE`).
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_BODY_MESSAGE: &str = "Invalid request body";

/// Both paths the two association operations are served on: a refusal that belongs to the
/// `Resource` member belongs to both of them.
const ASSOCIATION_PATHS: [&str; 2] =
    ["/associate-to-configuration", "/disassociate-from-configuration"];

/// The modelled 500 a target that cannot host the link is refused with, and the fixed
/// sentence it carries.
const INTERNAL_SERVICE_ERROR: &str = "InternalServiceError";
const INTERNAL_SERVER_ERROR_MESSAGE: &str = "Internal Server Error";

/// A router mounting only this service over one shared engine, with [`WORKSPACE`]
/// authorized.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    chatbot::state::authorize_slack_workspace(&*engine, WORKSPACE.0, WORKSPACE.1)
        .expect("the shared engine holds the chatbot store");
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(chatbot::EmulatedService::new(engine));
    router
}

/// Create the custom action `action_name` and answer the ARN it was minted under.
async fn given_custom_action(router: &MultiServiceRouter, action_name: &str) -> String {
    let answer = post(
        router,
        REGION,
        "/create-custom-action",
        &json!({
            "ActionName": action_name,
            "Definition": {"CommandText": "ssm list-documents"},
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "creating the custom action {action_name:?}: expected 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body["CustomActionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("creating {action_name:?}: no CustomActionArn in {}", answer.body))
        .to_string()
}

/// Create the Slack channel configuration `name` and answer its `ChatConfigurationArn`.
async fn given_slack_channel_configuration(router: &MultiServiceRouter, name: &str) -> String {
    let answer = post(
        router,
        REGION,
        "/create-slack-channel-configuration",
        &json!({
            "ConfigurationName": name,
            "SlackTeamId": WORKSPACE.0,
            "SlackChannelId": "C0123ABCD",
            "IamRoleArn": IAM_ROLE_ARN,
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "creating the Slack channel configuration {name:?}: expected 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body["ChannelConfiguration"]["ChatConfigurationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("creating {name:?}: no ChatConfigurationArn in {}", answer.body))
        .to_string()
}

/// Create the Amazon Chime webhook configuration `name` and answer its
/// `ChatConfigurationArn`.
async fn given_chime_webhook_configuration(router: &MultiServiceRouter, name: &str) -> String {
    let answer = post(
        router,
        REGION,
        "/create-chime-webhook-configuration",
        &json!({
            "ConfigurationName": name,
            "WebhookDescription": "chatbot emulator webhook",
            "WebhookUrl": "https://hooks.chime.aws/incomingwebhooks/abc-123?token=tok-456",
            "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:chatbot-topic"],
            "IamRoleArn": IAM_ROLE_ARN,
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "creating the Chime webhook configuration {name:?}: expected 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body["WebhookConfiguration"]["ChatConfigurationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("creating {name:?}: no ChatConfigurationArn in {}", answer.body))
        .to_string()
}

/// The resources `ListAssociations` reports for `chat_configuration`, in the order it
/// reports them, reading every page.
async fn associated_resources(
    router: &MultiServiceRouter,
    chat_configuration: &str,
    max_results: Option<i64>,
) -> Vec<String> {
    let mut resources = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({"ChatConfiguration": chat_configuration});
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let answer = post(router, REGION, "/list-associations", &request).await;
        assert_eq!(
            answer.status, 200,
            "listing associations of {chat_configuration:?}: expected 200, got {} with {}",
            answer.status, answer.body
        );
        let listed = answer.body["Associations"]
            .as_array()
            .unwrap_or_else(|| panic!("no Associations in {}", answer.body))
            .clone();
        for association in listed {
            resources.push(
                association["Resource"]
                    .as_str()
                    .unwrap_or_else(|| panic!("no Resource in {association}"))
                    .to_string(),
            );
        }
        next_token = answer.body["NextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            return resources;
        }
    }
}

#[tokio::test]
async fn a_resource_naming_no_action_of_this_account_is_unauthorized() {
    let router = router();
    let chat_configuration = given_slack_channel_configuration(&router, "assoc-auth").await;
    let chime = given_chime_webhook_configuration(&router, "assoc-auth-chime").await;
    // The action really exists, so nothing below is refused for want of a resource.
    let minted = given_custom_action(&router, "auth-probe").await;

    // Every row names a resource no custom action of this account could ever be minted
    // under, differing in HOW it fails to: the first four SATISFY `ResourceIdentifier`'s
    // modelled pattern
    // (`^arn:aws:chatbot:[A-Za-z0-9_/.-]{0,63}:[A-Za-z0-9_/.-]{0,63}:custom-action/[a-zA-Z0-9_-]{1,64}$`,
    // smithy_query query="operation" AssociateToConfiguration) and fall outside the
    // account's namespace — by the region segment (a custom action is account-scoped, so
    // the ARN a create mints carries none) or by the account id — while the rest fall
    // outside the pattern as well: a string that is no ARN, one below the member's @length
    // minimum, an ARN of another chatbot resource type, of this service's own
    // configurations, of another service and of another partition. The decision is about
    // the NAMESPACE the ARN addresses, so both halves are refused alike; what the ARN says
    // about the action's own NAME is a modelled constraint on the member and is left to it
    // (the test below).
    let rows: [(&str, String); 11] = [
        (
            "another account",
            "arn:aws:chatbot::210987654321:custom-action/auth-probe".to_string(),
        ),
        (
            "another region",
            format!("arn:aws:chatbot:{OTHER_REGION}:{ACCOUNT}:custom-action/never-created"),
        ),
        (
            "a region segment filled in, naming no action",
            format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:custom-action/never-created"),
        ),
        (
            "a region segment filled in, naming the action that exists",
            format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:custom-action/auth-probe"),
        ),
        (
            "a Resource that is not an ARN at all",
            "not-a-valid-arn-at-all".to_string(),
        ),
        (
            "a Resource below the @length minimum",
            String::new(),
        ),
        (
            "a Resource that is a channel configuration's own ARN",
            chat_configuration.clone(),
        ),
        (
            "a Resource that is a Chime webhook configuration's ARN",
            chime.clone(),
        ),
        (
            "a Resource of another chatbot resource type",
            format!("arn:aws:chatbot::{ACCOUNT}:chat-configuration/slack-channel/assoc-auth"),
        ),
        (
            "a Resource of another service",
            format!("arn:aws:lambda::{ACCOUNT}:custom-action/auth-probe"),
        ),
        (
            "a Resource of another partition",
            format!("arn:aws-cn:chatbot::{ACCOUNT}:custom-action/auth-probe"),
        ),
    ];

    for (case, resource) in &rows {
        for path in ASSOCIATION_PATHS {
            let answer = post(
                &router,
                REGION,
                path,
                &json!({"Resource": resource, "ChatConfiguration": chat_configuration}),
            )
            .await;
            assert_eq!(
                answer.status, 403,
                "case {case:?} on {path}: expected HTTP 403, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                UNAUTHORIZED_EXCEPTION,
                "case {case:?} on {path}: expected the modelled {UNAUTHORIZED_EXCEPTION}, got {}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                UNAUTHORIZED_MESSAGE,
                "case {case:?} on {path}: expected the fixed refusal sentence, got {}",
                answer.body
            );
            // restJson1 resolves the error SHAPE from this header, so it names the modelled
            // error too — not the framework's synthetic validation shape.
            assert_eq!(
                answer.error_type.as_deref(),
                Some(UNAUTHORIZED_EXCEPTION),
                "case {case:?} on {path}: expected x-amzn-errortype \
                 {UNAUTHORIZED_EXCEPTION}, got {:?}",
                answer.error_type
            );
        }
    }

    // The refusals linked nothing, and the ARN a create DOES mint is served on the same
    // configuration — so the 403s came from the target named and not from the call.
    assert_eq!(
        associated_resources(&router, &chat_configuration, None).await,
        Vec::<String>::new(),
        "a refused associate must link nothing"
    );
    let served = post(
        &router,
        REGION,
        "/associate-to-configuration",
        &json!({"Resource": minted, "ChatConfiguration": chat_configuration}),
    )
    .await;
    assert_eq!(
        served.status, 201,
        "the minted ARN is served, got {} with {}",
        served.status, served.body
    );
}

#[tokio::test]
async fn a_member_outside_its_modelled_constraints_is_refused_as_client_input() {
    let router = router();
    let chat_configuration = given_slack_channel_configuration(&router, "assoc-shape").await;
    // The action really exists, so every row below carries a `Resource` nothing else could
    // refuse: what is outside its constraints is the OTHER member.
    let minted = given_custom_action(&router, "shape-probe").await;

    // Each row puts ONE member outside the constraints the model declares for it, where
    // nothing about the NAMESPACE the request addresses is wrong — so the modelled
    // constraint is what decides the answer (smithy_query query="operation"
    // AssociateToConfiguration,DisassociateFromConfiguration):
    //   Resource -> ResourceIdentifier, @length 1..1011, @pattern
    //     ^arn:aws:chatbot:[A-Za-z0-9_/.-]{0,63}:[A-Za-z0-9_/.-]{0,63}:custom-action/[a-zA-Z0-9_-]{1,64}$
    //   ChatConfiguration -> ChatConfigurationArn, @length 19..1169, @pattern
    //     ^arn:aws:(wheatley|chatbot):[A-Za-z0-9_/.-]{0,63}:[A-Za-z0-9_/.-]{0,63}:[A-Za-z0-9][A-Za-z0-9:_/+=,@.-]{0,1023}$
    // The rows differ along what those constraints generalise over: WHICH member carries
    // the offending value, the action name's charset and its own length bound (both inside
    // the ARN form this account mints, so the authorization decision above admits them),
    // and for `ChatConfiguration` — which names no resource the call is authorized against
    // — its length bound, its partition, the service name its pattern admits, and the
    // leading character of its resource segment. The pattern's upper bound on the action
    // name (64) keeps every conforming value well under `Resource`'s @length maximum, so
    // there is no row past that maximum.
    let rows: [(&str, String, String); 7] = [
        (
            "a Resource whose action name is outside the published charset",
            format!("arn:aws:chatbot::{ACCOUNT}:custom-action/has spaces"),
            chat_configuration.clone(),
        ),
        (
            "a Resource whose action name is one character past the pattern's 64",
            format!("arn:aws:chatbot::{ACCOUNT}:custom-action/{}", "z".repeat(65)),
            chat_configuration.clone(),
        ),
        (
            "a ChatConfiguration below its @length minimum",
            minted.clone(),
            "arn:aws:chatbot".to_string(),
        ),
        (
            "a ChatConfiguration that is not an ARN at all",
            minted.clone(),
            "not-a-valid-arn-at-all".to_string(),
        ),
        (
            "a ChatConfiguration of another partition",
            minted.clone(),
            format!(
                "arn:aws-cn:chatbot:{REGION}:{ACCOUNT}:chat-configuration/slack-channel/assoc-shape"
            ),
        ),
        (
            "a ChatConfiguration of a service the pattern does not admit",
            minted.clone(),
            format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:chat-configuration/slack-channel/assoc-shape"
            ),
        ),
        (
            "a ChatConfiguration whose resource segment starts outside the pattern's charset",
            minted.clone(),
            format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:-chat-configuration/slack-channel/x"),
        ),
    ];

    for (case, resource, target) in &rows {
        for path in ASSOCIATION_PATHS {
            let answer = post(
                &router,
                REGION,
                path,
                &json!({"Resource": resource, "ChatConfiguration": target}),
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {case:?} on {path}: expected HTTP 400, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                INVALID_REQUEST_EXCEPTION,
                "case {case:?} on {path}: expected the modelled {INVALID_REQUEST_EXCEPTION}, \
                 got {}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                INVALID_REQUEST_BODY_MESSAGE,
                "case {case:?} on {path}: expected the fixed refusal sentence, got {}",
                answer.body
            );
            // restJson1 resolves the error SHAPE from this header, so it names the modelled
            // error too — not the framework's synthetic validation shape.
            assert_eq!(
                answer.error_type.as_deref(),
                Some(INVALID_REQUEST_EXCEPTION),
                "case {case:?} on {path}: expected x-amzn-errortype \
                 {INVALID_REQUEST_EXCEPTION}, got {:?}",
                answer.error_type
            );
        }
    }

    // None of those refusals linked anything, and the minted ARN is still served on the
    // same configuration — so the 400s came from the values and not from the call.
    assert_eq!(
        associated_resources(&router, &chat_configuration, None).await,
        Vec::<String>::new(),
        "a refused associate must link nothing"
    );
    let served = post(
        &router,
        REGION,
        "/associate-to-configuration",
        &json!({"Resource": minted, "ChatConfiguration": chat_configuration}),
    )
    .await;
    assert_eq!(
        served.status, 201,
        "the minted ARN is served, got {} with {}",
        served.status, served.body
    );
}

#[tokio::test]
async fn a_target_that_cannot_host_the_link_is_refused_and_links_nothing() {
    let router = router();
    let resource = given_custom_action(&router, "unsupported-target").await;
    let chime = given_chime_webhook_configuration(&router, "chime-target").await;

    // Each row is a `ChatConfiguration` that cannot host an association, differing in WHY:
    // the chat client it configures takes none, or this account holds no configuration
    // under the ARN at all (of either client, and in either region).
    let rows: [(&str, String); 4] = [
        ("an Amazon Chime webhook configuration", chime.clone()),
        (
            "a Slack channel configuration that was never created",
            format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/slack-channel/absent"),
        ),
        (
            "a Chime webhook configuration that was never created",
            format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/chime-webhook/absent"),
        ),
        (
            "a Slack channel configuration of another region",
            format!(
                "arn:aws:chatbot:{OTHER_REGION}:{ACCOUNT}:chat-configuration/slack-channel/absent"
            ),
        ),
    ];

    for (case, chat_configuration) in &rows {
        // Fired eight ways at once against the SAME pair: the refusal is a property of the
        // target, so every caller is refused alike and none of them half-links anything.
        let request = json!({"Resource": resource, "ChatConfiguration": chat_configuration});
        let answers = tokio::join!(
            post(&router, REGION, "/associate-to-configuration", &request),
            post(&router, REGION, "/associate-to-configuration", &request),
            post(&router, REGION, "/associate-to-configuration", &request),
            post(&router, REGION, "/associate-to-configuration", &request),
            post(&router, REGION, "/associate-to-configuration", &request),
            post(&router, REGION, "/associate-to-configuration", &request),
            post(&router, REGION, "/associate-to-configuration", &request),
            post(&router, REGION, "/associate-to-configuration", &request),
        );
        let answers = [
            answers.0, answers.1, answers.2, answers.3, answers.4, answers.5, answers.6, answers.7,
        ];
        for (index, answer) in answers.iter().enumerate() {
            assert_eq!(
                answer.status, 500,
                "case {case:?} call {index}: expected HTTP 500, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                INTERNAL_SERVICE_ERROR,
                "case {case:?} call {index}: expected the modelled {INTERNAL_SERVICE_ERROR}, \
                 got {}",
                answer.body
            );
            assert_eq!(
                answer.message_capitalized(),
                INTERNAL_SERVER_ERROR_MESSAGE,
                "case {case:?} call {index}: expected the fixed refusal sentence, got {}",
                answer.body
            );
        }
        assert_eq!(
            associated_resources(&router, chat_configuration, None).await,
            Vec::<String>::new(),
            "case {case:?}: a refused associate must leave no association behind"
        );
    }

    // Unlinking from such a target is NOT an error: the caller asked for the resource not
    // to be linked, and it is not.
    for (case, chat_configuration) in &rows {
        let answer = post(
            &router,
            REGION,
            "/disassociate-from-configuration",
            &json!({"Resource": resource, "ChatConfiguration": chat_configuration}),
        )
        .await;
        assert_eq!(
            answer.status, 204,
            "case {case:?}: a disassociate is an idempotent no-op, got {} with {}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn a_link_round_trips_through_list_associations_and_both_calls_are_idempotent() {
    let router = router();
    let chat_configuration = given_slack_channel_configuration(&router, "assoc-roundtrip").await;
    let other_configuration = given_slack_channel_configuration(&router, "assoc-other").await;

    // The action names differ in length and charset, so what is linked cannot follow from
    // the spelling of one ARN.
    let action_names = ["a", "linked-action", &"z".repeat(64), "Mixed_Case-9"];
    let mut resources = Vec::new();
    for action_name in action_names {
        resources.push(given_custom_action(&router, action_name).await);
    }

    // Each row links one more resource, and twice: an associate is idempotent, so the
    // expectation after it is the whole set linked so far, in one stable order.
    let mut expected: Vec<String> = Vec::new();
    for resource in &resources {
        for attempt in 0..2 {
            let answer = post(
                &router,
                REGION,
                "/associate-to-configuration",
                &json!({"Resource": resource, "ChatConfiguration": chat_configuration}),
            )
            .await;
            assert_eq!(
                answer.status, 201,
                "case {resource:?} attempt {attempt}: expected HTTP 201, got {} with {}",
                answer.status, answer.body
            );
            assert!(
                answer
                    .body
                    .as_object()
                    .is_none_or(|members| members.is_empty()),
                "case {resource:?} attempt {attempt}: the success carries no members, got {}",
                answer.body
            );
        }
        expected.push(resource.clone());
        expected.sort();
        assert_eq!(
            associated_resources(&router, &chat_configuration, None).await,
            expected,
            "case {resource:?}: expected every resource linked so far, exactly once"
        );
    }

    // The links belong to the configuration they were made against, not to the account.
    assert_eq!(
        associated_resources(&router, &other_configuration, None).await,
        Vec::<String>::new(),
        "another configuration holds none of these links"
    );

    // `MaxResults` splits the SAME total answer into pages, whatever the page size.
    for page_size in 1..=(expected.len() as i64 + 1) {
        assert_eq!(
            associated_resources(&router, &chat_configuration, Some(page_size)).await,
            expected,
            "case MaxResults={page_size}: paging must answer the same set in the same order"
        );
    }

    // Unlinking removes exactly one link, and a second unlink of the same resource is the
    // same empty success.
    for resource in &resources {
        for attempt in 0..2 {
            let answer = post(
                &router,
                REGION,
                "/disassociate-from-configuration",
                &json!({"Resource": resource, "ChatConfiguration": chat_configuration}),
            )
            .await;
            assert_eq!(
                answer.status, 204,
                "case {resource:?} attempt {attempt}: expected HTTP 204, got {} with {}",
                answer.status, answer.body
            );
        }
        expected.retain(|linked| linked != resource);
        assert_eq!(
            associated_resources(&router, &chat_configuration, None).await,
            expected,
            "case {resource:?}: expected only the remaining links"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_names_no_position_in_the_scan() {
    let router = router();
    let chat_configuration = given_slack_channel_configuration(&router, "assoc-tokens").await;
    let resource = given_custom_action(&router, "token-action").await;
    let associated = post(
        &router,
        REGION,
        "/associate-to-configuration",
        &json!({"Resource": resource, "ChatConfiguration": chat_configuration}),
    )
    .await;
    assert_eq!(
        associated.status, 201,
        "the associate answers 201, got {} with {}",
        associated.status, associated.body
    );

    // A cursor is one of this account's custom-action ARNs, base64-encoded. Each row is a
    // string that is not one, differing in how it fails to be: not base64, base64 of
    // something else, and a cursor of another listing of this service.
    let rows: [(&str, String); 4] = [
        ("not base64 at all", "!!!not-base64!!!".to_string()),
        (
            "base64 of a string that is no ARN",
            ::aws_smithy_types::base64::encode("page-2"),
        ),
        (
            "base64 of another account's action ARN",
            ::aws_smithy_types::base64::encode(
                "arn:aws:chatbot::210987654321:custom-action/token-action",
            ),
        ),
        (
            "a cursor of the configuration listing",
            ::aws_smithy_types::base64::encode(format!(
                "arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/slack-channel/assoc-tokens"
            )),
        ),
    ];
    for (case, token) in &rows {
        let answer = post(
            &router,
            REGION,
            "/list-associations",
            &json!({"ChatConfiguration": chat_configuration, "NextToken": token}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidRequestException",
            "case {case:?}: expected this service's client-input error class, got {}",
            answer.body
        );
    }

    // A configuration this account holds nothing under is not a refusal: it has no
    // association to report.
    let empty = associated_resources(
        &router,
        &format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/slack-channel/absent"),
        None,
    )
    .await;
    assert_eq!(
        empty,
        Vec::<String>::new(),
        "a configuration holding no association answers an empty list"
    );
}

/// Assert `answer` is this service's resource-scoped authorization refusal: the modelled
/// 403, the fixed sentence, and the header restJson1 resolves the error SHAPE from — so the
/// answer names the modelled error and not the framework's synthetic validation shape.
fn assert_unauthorized(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 403,
        "case {case}: expected HTTP 403, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        UNAUTHORIZED_EXCEPTION,
        "case {case}: expected the modelled {UNAUTHORIZED_EXCEPTION}, got {}",
        answer.body
    );
    assert_eq!(
        answer.message(),
        UNAUTHORIZED_MESSAGE,
        "case {case}: expected the fixed refusal sentence, got {}",
        answer.body
    );
    assert_eq!(
        answer.error_type.as_deref(),
        Some(UNAUTHORIZED_EXCEPTION),
        "case {case}: expected x-amzn-errortype {UNAUTHORIZED_EXCEPTION}, got {:?}",
        answer.error_type
    );
}

/// Assert `answer` is the request-boundary refusal of a value outside a member's modelled
/// constraints: this service's client-input class, its fixed sentence, and the same header.
fn assert_invalid_request(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected HTTP 400, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        INVALID_REQUEST_EXCEPTION,
        "case {case}: expected the modelled {INVALID_REQUEST_EXCEPTION}, got {}",
        answer.body
    );
    assert_eq!(
        answer.message(),
        INVALID_REQUEST_BODY_MESSAGE,
        "case {case}: expected the fixed refusal sentence, got {}",
        answer.body
    );
    assert_eq!(
        answer.error_type.as_deref(),
        Some(INVALID_REQUEST_EXCEPTION),
        "case {case}: expected x-amzn-errortype {INVALID_REQUEST_EXCEPTION}, got {:?}",
        answer.error_type
    );
}

#[tokio::test]
async fn the_resource_decision_precedes_member_constraints_for_every_target_kind() {
    let router = router();
    // The dimension this test spans is the KIND of chat configuration the unlink addresses,
    // which the tables above hold fixed at a Slack channel configuration: the two kinds this
    // model can create, and an ARN inside `ChatConfigurationArn`'s pattern this account holds
    // no configuration under. The decision below is taken on `Resource` — the member naming
    // the resource the call addresses — before any target is looked up, so none of the three
    // may change it.
    let chime = given_chime_webhook_configuration(&router, "disassoc-order-chime").await;
    let slack = given_slack_channel_configuration(&router, "disassoc-order-slack").await;
    let unknown =
        format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/slack-channel/never-made");
    // The configurations this account HOLDS, which is as far as the success half below goes:
    // what an unlink answers against a configuration that does not exist is not recorded.
    let held: [(&str, &str); 2] = [
        ("a Chime webhook configuration", chime.as_str()),
        ("a Slack channel configuration", slack.as_str()),
    ];
    let targets: [(&str, &str); 3] = [
        held[0],
        held[1],
        ("an ARN no configuration is stored under", unknown.as_str()),
    ];

    // Each row names a `Resource` no custom action of this account could be minted under,
    // differing in HOW it falls outside that namespace: a string that is no ARN at all
    // (outside `ResourceIdentifier`'s @pattern as well), another account's id, a region
    // segment filled in where an account-scoped action carries none, and another service's
    // namespace.
    let unauthorized: [(&str, String); 4] = [
        ("a Resource that is not an ARN at all", "not-a-valid-arn-at-all".to_string()),
        (
            "a Resource of another account",
            "arn:aws:chatbot::210987654321:custom-action/order-probe".to_string(),
        ),
        (
            "a Resource with a region segment filled in",
            format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:custom-action/order-probe"),
        ),
        (
            "a Resource of another service",
            format!("arn:aws:lambda::{ACCOUNT}:custom-action/order-probe"),
        ),
    ];

    // Inside the namespace this account mints actions under, the member's own modelled
    // constraints still decide: the action NAME is a property of the name rather than of the
    // namespace, so one outside the pattern's `[a-zA-Z0-9_-]{1,64}` stays client input
    // (smithy_query query="operation" operation="DisassociateFromConfiguration").
    let out_of_pattern_name = format!("arn:aws:chatbot::{ACCOUNT}:custom-action/order probe");

    for (target_case, target) in targets {
        for (case, resource) in &unauthorized {
            let answer = post(
                &router,
                REGION,
                "/disassociate-from-configuration",
                &json!({"Resource": resource, "ChatConfiguration": target}),
            )
            .await;
            assert_unauthorized(&answer, &format!("{case:?} against {target_case}"));
        }
        let refused = post(
            &router,
            REGION,
            "/disassociate-from-configuration",
            &json!({"Resource": out_of_pattern_name, "ChatConfiguration": target}),
        )
        .await;
        assert_invalid_request(
            &refused,
            &format!("an action name outside the pattern against {target_case}"),
        );
    }

    // The other half of the contract, on each configuration this account holds: a well-formed
    // custom-action ARN the configuration was never associated with unlinks without error —
    // the caller asked for it to be unlinked, and afterwards it is — and the configuration
    // lists no association for it.
    let never_associated = given_custom_action(&router, "order-control").await;
    for (target_case, target) in held {
        for attempt in 0..2 {
            let answer = post(
                &router,
                REGION,
                "/disassociate-from-configuration",
                &json!({"Resource": never_associated, "ChatConfiguration": target}),
            )
            .await;
            assert_eq!(
                answer.status, 204,
                "case a never-associated resource against {target_case} attempt {attempt}: \
                 expected HTTP 204, got {} with {}",
                answer.status, answer.body
            );
            assert!(
                answer
                    .body
                    .as_object()
                    .is_none_or(|members| members.is_empty()),
                "case a never-associated resource against {target_case} attempt {attempt}: \
                 the success carries no members, got {}",
                answer.body
            );
        }
        assert_eq!(
            associated_resources(&router, target, None).await,
            Vec::<String>::new(),
            "case {target_case}: a no-op unlink lists no association"
        );
    }

    // And the no-op removes nothing that IS linked: the configuration that hosts links keeps
    // the resource associated with it while another, never-associated one is unlinked.
    let linked = given_custom_action(&router, "order-linked").await;
    let associated = post(
        &router,
        REGION,
        "/associate-to-configuration",
        &json!({"Resource": linked, "ChatConfiguration": slack}),
    )
    .await;
    assert_eq!(
        associated.status, 201,
        "linking the control resource: expected HTTP 201, got {} with {}",
        associated.status, associated.body
    );
    let no_op = post(
        &router,
        REGION,
        "/disassociate-from-configuration",
        &json!({"Resource": never_associated, "ChatConfiguration": slack}),
    )
    .await;
    assert_eq!(
        no_op.status, 204,
        "unlinking a never-associated resource: expected HTTP 204, got {} with {}",
        no_op.status, no_op.body
    );
    assert_eq!(
        associated_resources(&router, &slack, None).await,
        vec![linked],
        "a no-op unlink leaves every other link in place"
    );
}

#[tokio::test]
async fn the_resource_decision_precedes_the_members_length_bounds() {
    let router = router();
    // The dimension this test spans is the LENGTH of the `Resource` the unlink names: the
    // resource-scoped authorization decision precedes EVERY modelled constraint on that
    // member, its `@length` as much as its `@pattern`, so a value naming no custom action
    // this account could mint is refused alike at each published bound of
    // `ResourceIdentifier`'s @length (min 1, max 1011; smithy_query query="operation"
    // operation="DisassociateFromConfiguration") and one value past each.
    let chime = given_chime_webhook_configuration(&router, "disassoc-bounds-chime").await;
    let slack = given_slack_channel_configuration(&router, "disassoc-bounds-slack").await;
    // A link that really is there, so a refusal can be seen to remove nothing. It is made
    // against the Slack channel configuration because a Chime webhook configuration hosts no
    // link at all (the table above), which leaves the Chime target holding none.
    let linked = given_custom_action(&router, "bounds-linked").await;
    let associated = post(
        &router,
        REGION,
        "/associate-to-configuration",
        &json!({"Resource": linked, "ChatConfiguration": slack}),
    )
    .await;
    assert_eq!(
        associated.status, 201,
        "linking the control resource: expected HTTP 201, got {} with {}",
        associated.status, associated.body
    );

    // Each length is spelled two ways, so what is refused cannot follow from one spelling:
    // a value that is no ARN at all, and a custom-action ARN of this service padded to that
    // length whose region segment is filled in — a custom action is account-scoped, so the
    // ARN a create mints carries none and no action of this account could ever be minted
    // under it. The two shortest lengths admit only the first spelling, the ARN prefix
    // alone being longer than that.
    let prefix = format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:custom-action/");
    let mut rows: Vec<(String, String)> = Vec::new();
    for length in [0usize, 1, 1011, 1012] {
        rows.push((
            format!("a Resource of {length} characters that is no ARN at all"),
            "x".repeat(length),
        ));
        if let Some(padding) = length.checked_sub(prefix.len()).filter(|pad| *pad > 0) {
            rows.push((
                format!("a foreign custom-action ARN of {length} characters"),
                format!("{prefix}{}", "a".repeat(padding)),
            ));
        }
    }

    let targets: [(&str, &str); 2] = [
        ("a Chime webhook configuration", chime.as_str()),
        ("a Slack channel configuration", slack.as_str()),
    ];
    for (case, resource) in &rows {
        for (target_case, target) in targets {
            let answer = post(
                &router,
                REGION,
                "/disassociate-from-configuration",
                &json!({"Resource": resource, "ChatConfiguration": target}),
            )
            .await;
            assert_unauthorized(&answer, &format!("{case:?} against {target_case}"));
        }
    }

    // None of those refusals touched the store: the link made above is still reported, and
    // the configuration that never held one still holds none.
    assert_eq!(
        associated_resources(&router, &slack, None).await,
        vec![linked],
        "a refused unlink removes no link the configuration holds"
    );
    assert_eq!(
        associated_resources(&router, &chime, None).await,
        Vec::<String>::new(),
        "a refused unlink leaves a configuration holding no link empty"
    );
}

#[tokio::test]
async fn a_never_associated_resource_unlinks_as_a_no_op_across_the_action_name_bounds() {
    let router = router();
    // The dimension this test spans is the SPELLING of the well-formed `Resource` a no-op
    // unlink names: the operation is @idempotent with a 204 success code, so an in-namespace
    // custom-action ARN this configuration was never associated with answers the empty
    // success whatever the action name is — and removes nothing else.
    let chime = given_chime_webhook_configuration(&router, "disassoc-noop-chime").await;
    let slack = given_slack_channel_configuration(&router, "disassoc-noop-slack").await;
    let linked = given_custom_action(&router, "noop-linked").await;
    let associated = post(
        &router,
        REGION,
        "/associate-to-configuration",
        &json!({"Resource": linked, "ChatConfiguration": slack}),
    )
    .await;
    assert_eq!(
        associated.status, 201,
        "linking the control resource: expected HTTP 201, got {} with {}",
        associated.status, associated.body
    );

    // The names span the published bounds and charset of the action name
    // (`CustomActionName`, @length 1..64, @pattern ^[a-zA-Z0-9_-]{1,64}$; smithy_query
    // query="operation" operation="CreateCustomAction"): the length minimum, the length
    // maximum, and the upper case, digits, underscore and hyphen it admits.
    let longest = "n".repeat(64);
    let names: [&str; 5] = ["q", longest.as_str(), "Mixed_Case-9", "9", "_-under-score_"];

    // Both kinds of target this model can create: the one holding a link, and one holding
    // none. Neither hosts a link for the resources below, so every call is a no-op.
    let targets: [(&str, &str); 2] = [
        ("a Slack channel configuration", slack.as_str()),
        ("a Chime webhook configuration", chime.as_str()),
    ];
    for name in names {
        let never_associated = given_custom_action(&router, name).await;
        for (target_case, target) in targets {
            for attempt in 0..2 {
                let answer = post(
                    &router,
                    REGION,
                    "/disassociate-from-configuration",
                    &json!({"Resource": never_associated, "ChatConfiguration": target}),
                )
                .await;
                assert_eq!(
                    answer.status, 204,
                    "case {name:?} against {target_case} attempt {attempt}: expected HTTP 204, \
                     got {} with {}",
                    answer.status, answer.body
                );
                assert!(
                    answer
                        .body
                        .as_object()
                        .is_none_or(|members| members.is_empty()),
                    "case {name:?} against {target_case} attempt {attempt}: the success carries \
                     no members, got {}",
                    answer.body
                );
            }
        }
        assert_eq!(
            associated_resources(&router, &slack, None).await,
            vec![linked.clone()],
            "case {name:?}: a no-op unlink leaves the link that IS there in place"
        );
        assert_eq!(
            associated_resources(&router, &chime, None).await,
            Vec::<String>::new(),
            "case {name:?}: a no-op unlink links nothing to a configuration holding none"
        );
    }
}
