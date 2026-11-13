// SPDX-License-Identifier: CC0-1.0

//! The WhatsApp resources this service holds behind Meta — Flows, message templates,
//! business public keys and media objects — end to end over `restJson1`.
//!
//! RULE: every one of these operations RESOLVES the resource its request names against the
//! store before it answers. A name that resolves to nothing is refused 404
//! `ResourceNotFoundException` carrying the constant that lookup states — the `id`-scoped
//! Meta resources (the business account, and the Flow, template or media object held under
//! it) state one text, the phone-number-scoped ones another — and a name that resolves is
//! answered from the stored resource, so a create is what a read reports and a delete is
//! what a second delete misses.
//!
//! The rows span what the rule generalises over: which operation performs the lookup, which
//! FORM the id arrives in (the short form, the resource ARN, the shortest and longest the
//! modelled bound allows), whether the miss is on the account or on the resource under an
//! account that does exist, and — on the success side — the values each create can take.

use aws_smithy_types::base64;
use serde_json::{json, Value};
use service_com_amazonaws_socialmessaging_socialmessaging::state::{
    ComAmazonawsSocialmessagingSocialmessagingState as Store, MediaObject,
    TEMPLATE_STATUS_APPROVED, TEMPLATE_STATUS_PENDING,
};
use smithy_server_runtime::MultiServiceRouter;
use std::time::SystemTime;

use crate::harness::{
    call, emulator, emulator_with_engine, link, query_escape, rsa_public_key_pem, Answer,
    ACCOUNT_ID, INVALID_PARAMETERS_ERROR_TYPE,
};

/// The message the `id`-scoped lookups answer a miss with
/// (`ID_NOT_FOUND_FOR_ACCOUNT_MESSAGE` in `src/state.rs`).
const ID_NOT_FOUND: &str = "Id not found for account.";

/// The message the phone-number lookups answer a miss with
/// (`ORIGINATION_PHONE_NUMBER_NOT_FOUND_MESSAGE` in `src/state.rs`) — different text, so a
/// caller can tell which lookup failed.
const ORIGINATION_NOT_FOUND: &str = "Origination phone number id not found for account.";

/// A numeric value satisfying `MetaFlowId` / `MetaTemplateId` (`^[0-9]+$`, `len 1..100`)
/// that names no Flow and no template.
const UNKNOWN_NUMERIC_ID: &str = "123456789012";

/// An alphanumeric value satisfying `WhatsAppMediaId` (`^[A-Za-z0-9]+$`, `len 1..100`) that
/// names no media object.
const UNKNOWN_MEDIA_ID: &str = "abcDEF123";

/// A KMS key ARN satisfying `KmsKeyArn`'s pattern, for the rows that set a key held in KMS.
fn kms_key_arn() -> String {
    format!("arn:aws:kms:us-east-1:{ACCOUNT_ID}:key/1234abcd-12ab-34cd-56ef-1234567890ab")
}


async fn get_flow(router: &MultiServiceRouter, id: &str, flow_id: &str) -> Answer {
    let uri = format!(
        "/v1/whatsapp/flow?id={}&flowId={}",
        query_escape(id),
        query_escape(flow_id)
    );
    call(router, "GET", &uri, None).await
}

async fn create_flow(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/v1/whatsapp/flow/create", Some(body)).await
}

async fn create_template(router: &MultiServiceRouter, id: &str, definition: &str) -> Answer {
    call(
        router,
        "POST",
        "/v1/whatsapp/template/put",
        Some(json!({ "id": id, "templateDefinition": base64::encode(definition) })),
    )
    .await
}

async fn get_template(router: &MultiServiceRouter, query: &str) -> Answer {
    call(router, "GET", &format!("/v1/whatsapp/template{query}"), None).await
}

async fn get_public_key(router: &MultiServiceRouter, phone_number_id: &str) -> Answer {
    let uri = format!(
        "/v1/whatsapp/business-public-key?originationPhoneNumberId={}",
        query_escape(phone_number_id)
    );
    call(router, "GET", &uri, None).await
}

async fn put_public_key(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "PUT", "/v1/whatsapp/business-public-key", Some(body)).await
}

async fn delete_media(router: &MultiServiceRouter, media_id: &str, phone_number_id: &str) -> Answer {
    let uri = format!(
        "/v1/whatsapp/media?mediaId={}&originationPhoneNumberId={}",
        query_escape(media_id),
        query_escape(phone_number_id)
    );
    call(router, "DELETE", &uri, None).await
}

/// The minted phone-number id of the first number registered under `id`, read back through
/// the account read — the only way a caller learns it.
async fn first_phone_number_id(router: &MultiServiceRouter, id: &str) -> String {
    let uri = format!("/v1/whatsapp/waba/details?id={}", query_escape(id));
    let account = call(router, "GET", &uri, None).await;
    assert_eq!(
        account.status, 200,
        "setup: expected 200 from GetLinkedWhatsAppBusinessAccount, got {} {:?}",
        account.status, account.body
    );
    account.string_at("/account/phoneNumbers/0/phoneNumberId")
}

/// Assert one answer is the 404 `ResourceNotFoundException` carrying `expected`.
fn assert_not_found(case: &str, answer: &Answer, expected: &str) {
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (404, "ResourceNotFoundException"),
        "case {case}: expected 404 ResourceNotFoundException, got {} {:?} {:?}",
        answer.status,
        answer.error_type,
        answer.body
    );
    let actual = answer.message();
    assert_eq!(
        actual, expected,
        "case {case}: expected the message {expected:?}, got {actual:?}"
    );
}

/// RULE, the refusal half: each of these operations resolves the resource the request names
/// and refuses a miss with its own constant — whatever FORM the id arrived in, and whether
/// the account itself or only the resource under it is missing.
#[tokio::test]
async fn every_resource_lookup_refuses_a_miss_with_its_lookups_constant() {
    let router = emulator();
    // One account with one number exists, so a miss is a miss on the name the request
    // carried and not on an empty store.
    let linked = link(
        &router,
        "waba-55555555555555555555555555555555",
        &["meta-phone-1"],
    )
    .await;
    let phone_number_id = first_phone_number_id(&router, &linked).await;

    // Ids that name no resource: the short form, the ARN form, the shortest the `1..115`
    // bound allows and the longest.
    let unknown_wabas = [
        "waba-00000000000000000000000000000000".to_string(),
        format!("arn:aws:social-messaging:us-east-1:{ACCOUNT_ID}:waba/0000000000000000"),
        "waba-".to_string(),
        format!("waba-{}", "0".repeat(110)),
    ];
    let unknown_phones = [
        "phone-number-id-00000000000000000000000000000000".to_string(),
        format!("arn:aws:social-messaging:us-east-1:{ACCOUNT_ID}:phone-number-id/00000000"),
        "phone-number-id-".to_string(),
        format!("phone-number-id-{}", "0".repeat(99)),
    ];

    for waba in &unknown_wabas {
        let misses = [
            ("GetWhatsAppFlow", get_flow(&router, waba, UNKNOWN_NUMERIC_ID).await),
            (
                "CreateWhatsAppFlow",
                create_flow(
                    &router,
                    json!({
                        "id": waba,
                        "flowName": "sim-flow",
                        "categories": vec!["OTHER"; 9],
                    }),
                )
                .await,
            ),
            (
                "CreateWhatsAppMessageTemplate",
                create_template(&router, waba, r#"{"name":"x"}"#).await,
            ),
            (
                "GetWhatsAppMessageTemplate",
                get_template(
                    &router,
                    &format!(
                        "?id={}&metaTemplateId={}",
                        query_escape(waba),
                        UNKNOWN_NUMERIC_ID
                    ),
                )
                .await,
            ),
        ];
        for (operation, answer) in misses {
            assert_not_found(&format!("{operation} on {waba:?}"), &answer, ID_NOT_FOUND);
        }
    }

    // The same miss one level down: the account exists, the resource under it does not.
    let under_the_account = [
        (
            "GetWhatsAppFlow for an unknown flow",
            get_flow(&router, &linked, UNKNOWN_NUMERIC_ID).await,
        ),
        (
            "CreateWhatsAppFlow cloning an unknown flow",
            create_flow(
                &router,
                json!({
                    "id": linked,
                    "flowName": "cloned-from-nothing",
                    "categories": ["OTHER"],
                    "cloneFlowId": UNKNOWN_NUMERIC_ID,
                }),
            )
            .await,
        ),
        (
            "GetWhatsAppMessageTemplate for an unknown template id",
            get_template(
                &router,
                &format!(
                    "?id={}&metaTemplateId={}",
                    query_escape(&linked),
                    UNKNOWN_NUMERIC_ID
                ),
            )
            .await,
        ),
        (
            "GetWhatsAppMessageTemplate for an unknown template name",
            get_template(
                &router,
                &format!("?id={}&templateName=no-such-template", query_escape(&linked)),
            )
            .await,
        ),
        (
            "DeleteWhatsAppMessageMedia for an unknown media id",
            delete_media(&router, UNKNOWN_MEDIA_ID, &phone_number_id).await,
        ),
    ];
    for (case, answer) in under_the_account {
        assert_not_found(case, &answer, ID_NOT_FOUND);
    }

    for phone in &unknown_phones {
        let misses = [
            (
                "GetWhatsAppBusinessPublicKey",
                get_public_key(&router, phone).await,
            ),
            (
                "PutWhatsAppBusinessPublicKey with a KMS key",
                put_public_key(
                    &router,
                    json!({
                        "originationPhoneNumberId": phone,
                        "kmsKeyArn": kms_key_arn(),
                    }),
                )
                .await,
            ),
            (
                "PutWhatsAppBusinessPublicKey with a PEM",
                put_public_key(
                    &router,
                    json!({
                        "originationPhoneNumberId": phone,
                        "businessPublicKey": rsa_public_key_pem(2048),
                    }),
                )
                .await,
            ),
            (
                "DeleteWhatsAppMessageMedia",
                delete_media(&router, UNKNOWN_MEDIA_ID, phone).await,
            ),
        ];
        for (operation, answer) in misses {
            assert_not_found(
                &format!("{operation} on {phone:?}"),
                &answer,
                ORIGINATION_NOT_FOUND,
            );
        }
    }
}

/// RULE, the ordering half: the request's SYNTAX is judged before any resource is resolved,
/// so a malformed value is refused 400 whether or not the resource it names exists.
#[tokio::test]
async fn a_malformed_name_is_refused_before_the_lookup_runs() {
    let router = emulator();
    let linked = link(&router, "waba-66666666666666666666666666666666", &["meta-phone-1"]).await;

    let rows = [
        (
            "a non-numeric flowId on a linked account",
            get_flow(&router, &linked, "12ab").await,
        ),
        (
            "a non-numeric metaTemplateId on a linked account",
            get_template(
                &router,
                &format!("?id={}&metaTemplateId=12ab", query_escape(&linked)),
            )
            .await,
        ),
        (
            "a mediaId outside the alphanumeric set, with an unknown number",
            delete_media(&router, "bad id!", "phone-number-id-0000").await,
        ),
        (
            "a malformed kmsKeyArn, with an unknown number",
            put_public_key(
                &router,
                json!({
                    "originationPhoneNumberId": "phone-number-id-0000",
                    "kmsKeyArn": "arn:aws:s3:::a-bucket",
                }),
            )
            .await,
        ),
        (
            "ten categories, one past the modelled maximum",
            create_flow(
                &router,
                json!({
                    "id": linked,
                    "flowName": "too-many-categories",
                    "categories": vec!["OTHER"; 10],
                }),
            )
            .await,
        ),
    ];
    for (case, answer) in rows {
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException, got {} {:?} {:?}",
            answer.status,
            answer.error_type,
            answer.body
        );
    }
}

/// RULE, the success half for Flows: a created Flow is read back as it was created — its
/// name, its categories, its status and the versions its definition declared.
///
/// The rows differ along what the create can state: how many categories (one, and the
/// modelled maximum of nine), whether a definition was handed in, whether the create asked
/// to publish, and whether an endpoint was named. `CreateWhatsAppFlow` documents the status
/// rule: "the Flow is created in DRAFT status. If `publish` is set to `true` and a valid
/// `flowJson` is provided, the Flow is published immediately."
#[tokio::test]
async fn a_created_flow_is_read_back_as_it_was_created() {
    let router = emulator();
    let linked = link(&router, "waba-77777777777777777777777777777777", &["meta-phone-1"]).await;
    let definition = r#"{"version":"7.3","data_api_version":"3.0","screens":[]}"#;

    // (case, body, expected status, expected jsonVersion, expected dataApiVersion)
    let rows = vec![
        (
            "one category, no definition",
            json!({ "id": linked, "flowName": "flow-a", "categories": ["SIGN_UP"] }),
            "DRAFT",
            Value::Null,
            Value::Null,
        ),
        (
            "nine categories, the modelled maximum",
            json!({
                "id": linked,
                "flowName": "flow-b",
                "categories": [
                    "SIGN_UP", "SIGN_IN", "APPOINTMENT_BOOKING", "LEAD_GENERATION",
                    "SHOPPING", "CONTACT_US", "CUSTOMER_SUPPORT", "SURVEY", "OTHER",
                ],
            }),
            "DRAFT",
            Value::Null,
            Value::Null,
        ),
        (
            "a definition, not published",
            json!({
                "id": linked,
                "flowName": "flow-c",
                "categories": ["OTHER"],
                "flowJson": base64::encode(definition),
            }),
            "DRAFT",
            json!("7.3"),
            json!("3.0"),
        ),
        (
            "a definition, published",
            json!({
                "id": linked,
                "flowName": "flow-d",
                "categories": ["OTHER"],
                "flowJson": base64::encode(definition),
                "publish": true,
                "endpointUri": "https://example.com/flow",
            }),
            "PUBLISHED",
            json!("7.3"),
            json!("3.0"),
        ),
        (
            "published asked for with no definition to publish",
            json!({
                "id": linked,
                "flowName": "flow-e",
                "categories": ["OTHER"],
                "publish": true,
            }),
            "DRAFT",
            Value::Null,
            Value::Null,
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, body, expected_status, json_version, data_api_version) in rows {
        let flow_name = body["flowName"].as_str().unwrap_or_default().to_string();
        let categories = body["categories"].clone();
        let endpoint_uri = body
            .get("endpointUri")
            .cloned()
            .unwrap_or(Value::Null);
        let created = create_flow(&router, body).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200 from CreateWhatsAppFlow, got {} {:?}",
            created.status, created.body
        );
        let flow_id = created.string_at("/flowId");
        assert!(
            !flow_id.is_empty() && flow_id.bytes().all(|byte| byte.is_ascii_digit()),
            "case {case}: expected a numeric MetaFlowId, got {flow_id:?}"
        );
        assert!(
            !minted.contains(&flow_id),
            "case {case}: expected a Flow id of its own, got {flow_id:?} again"
        );
        minted.push(flow_id.clone());

        let read = get_flow(&router, &linked, &flow_id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetWhatsAppFlow, got {} {:?}",
            read.status, read.body
        );
        let actual = (
            read.string_at("/flowId"),
            read.string_at("/flowName"),
            read.string_at("/flowStatus"),
            read.at("/categories").clone(),
            read.at("/jsonVersion").clone(),
            read.at("/dataApiVersion").clone(),
            read.at("/endpointUri").clone(),
            read.string_at("/whatsAppBusinessAccount/id"),
        );
        let expected = (
            flow_id.clone(),
            flow_name,
            expected_status.to_string(),
            categories,
            json_version,
            data_api_version,
            endpoint_uri,
            "waba-77777777777777777777777777777777".to_string(),
        );
        assert_eq!(
            actual, expected,
            "case {case}: expected the Flow as created, got {:?}",
            read.body
        );
    }

    // A clone takes the source's definition: the clone is publishable although this call
    // handed in no `flowJson` of its own.
    let source = minted
        .get(3)
        .cloned()
        .expect("the published Flow of the rows above");
    let cloned = create_flow(
        &router,
        json!({
            "id": linked,
            "flowName": "flow-cloned",
            "categories": ["OTHER"],
            "cloneFlowId": source,
            "publish": true,
        }),
    )
    .await;
    assert_eq!(
        cloned.status, 200,
        "case a clone of a Flow with a definition: expected 200, got {} {:?}",
        cloned.status, cloned.body
    );
    let clone_read = get_flow(&router, &linked, &cloned.string_at("/flowId")).await;
    assert_eq!(
        (
            clone_read.string_at("/flowStatus"),
            clone_read.string_at("/jsonVersion"),
            clone_read.string_at("/flowName"),
        ),
        (
            "PUBLISHED".to_string(),
            "7.3".to_string(),
            "flow-cloned".to_string()
        ),
        "case a clone of a Flow with a definition: expected the source's definition facts \
         under this call's name, got {:?}",
        clone_read.body
    );

    // "The name of the Flow. Must be unique within the WhatsApp Business Account."
    let duplicate = create_flow(
        &router,
        json!({ "id": linked, "flowName": "flow-a", "categories": ["OTHER"] }),
    )
    .await;
    assert_eq!(
        (duplicate.status, duplicate.error_type.as_str()),
        (400, INVALID_PARAMETERS_ERROR_TYPE),
        "case a name the account already holds a Flow under: expected 400 \
         {INVALID_PARAMETERS_ERROR_TYPE:?}, got {} {:?} {:?}",
        duplicate.status,
        duplicate.error_type,
        duplicate.body
    );

    // A Flow belongs to the account that created it: another account's Flow is a miss.
    let other = link(&router, "waba-88888888888888888888888888888888", &["meta-phone-2"]).await;
    let across = get_flow(&router, &other, &minted[0]).await;
    assert_not_found("another account's Flow", &across, ID_NOT_FOUND);
}

/// RULE, the success half for message templates: a submitted template is read back by the
/// id Meta assigned it and by the name its definition declared, with the definition the
/// create supplied.
///
/// The rows differ along what a definition can state (a name alone; a name, language and
/// category; a definition at the modelled `1..6000` maximum) and along how the read names
/// it (by id, by name, by name and language).
#[tokio::test]
async fn a_submitted_template_is_read_back_by_id_and_by_name() {
    let (router, engine) = emulator_with_engine();
    let linked = link(&router, "waba-99999999999999999999999999999999", &["meta-phone-1"]).await;

    // A definition exactly at `MetaTemplateDefinition`'s 6000-byte maximum.
    let padding = 6000 - r#"{"name":"at-the-bound","language":"en_US","body":""}"#.len();
    let at_bound = format!(
        r#"{{"name":"at-the-bound","language":"en_US","body":"{}"}}"#,
        "x".repeat(padding)
    );

    // (case, definition, expected category)
    let rows = vec![
        ("a name alone", r#"{"name":"name-only"}"#.to_string(), Value::Null),
        (
            "a name, language and category",
            r#"{"name":"greeting","language":"en_US","category":"UTILITY"}"#.to_string(),
            json!("UTILITY"),
        ),
        ("a definition at the modelled maximum", at_bound, Value::Null),
    ];

    for (case, definition, expected_category) in rows {
        let created = create_template(&router, &linked, &definition).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200 from CreateWhatsAppMessageTemplate, got {} {:?}",
            created.status, created.body
        );
        let meta_template_id = created.string_at("/metaTemplateId");
        assert!(
            !meta_template_id.is_empty()
                && meta_template_id.bytes().all(|byte| byte.is_ascii_digit()),
            "case {case}: expected a numeric MetaTemplateId, got {meta_template_id:?}"
        );
        assert_eq!(
            (
                created.string_at("/templateStatus"),
                created.at("/category").clone()
            ),
            (TEMPLATE_STATUS_PENDING.to_string(), expected_category),
            "case {case}: expected a template submitted for review, got {:?}",
            created.body
        );

        let by_id = get_template(
            &router,
            &format!(
                "?id={}&metaTemplateId={}",
                query_escape(&linked),
                query_escape(&meta_template_id)
            ),
        )
        .await;
        assert_eq!(
            (by_id.status, by_id.string_at("/template")),
            (200, definition.clone()),
            "case {case}, read by id: expected the definition as created, got {} {:?}",
            by_id.status,
            by_id.body
        );

        // The same template, named the way a caller that kept no id names it.
        let name = serde_json::from_str::<Value>(&definition)
            .ok()
            .and_then(|parsed| parsed["name"].as_str().map(str::to_string))
            .expect("every row's definition declares a name");
        let by_name = get_template(
            &router,
            &format!(
                "?id={}&templateName={}",
                query_escape(&linked),
                query_escape(&name)
            ),
        )
        .await;
        assert_eq!(
            (by_name.status, by_name.string_at("/template")),
            (200, definition.clone()),
            "case {case}, read by name: expected the definition as created, got {} {:?}",
            by_name.status,
            by_name.body
        );

        // And named by the name and the language together, which is how Meta identifies a
        // template: the language the definition declared resolves it, another does not.
        let language = serde_json::from_str::<Value>(&definition)
            .ok()
            .and_then(|parsed| parsed["language"].as_str().map(str::to_string));
        if let Some(language) = language {
            let matched = get_template(
                &router,
                &format!(
                    "?id={}&templateName={}&templateLanguageCode={}",
                    query_escape(&linked),
                    query_escape(&name),
                    query_escape(&language)
                ),
            )
            .await;
            assert_eq!(
                (matched.status, matched.string_at("/template")),
                (200, definition.clone()),
                "case {case}, read by name and language: expected the definition as \
                 created, got {} {:?}",
                matched.status,
                matched.body
            );
            let mismatched = get_template(
                &router,
                &format!(
                    "?id={}&templateName={}&templateLanguageCode=fr",
                    query_escape(&linked),
                    query_escape(&name)
                ),
            )
            .await;
            assert_not_found(
                &format!("{case}, read under another language"),
                &mismatched,
                ID_NOT_FOUND,
            );
        }
    }

    // A read that names neither an id nor a name names no template at all.
    let unnamed = get_template(&router, &format!("?id={}", query_escape(&linked))).await;
    assert_eq!(
        (unnamed.status, unnamed.error_type.as_str()),
        (400, INVALID_PARAMETERS_ERROR_TYPE),
        "case a read naming neither an id nor a name: expected 400 \
         {INVALID_PARAMETERS_ERROR_TYPE:?}, got {} {:?} {:?}",
        unnamed.status,
        unnamed.error_type,
        unnamed.body
    );

    // A definition that defines no template is a parameter that is not valid.
    for (case, definition) in [
        ("a definition that is not JSON", "not json at all"),
        ("a JSON array rather than an object", r#"["name"]"#),
        ("an object declaring no name", r#"{"language":"en_US"}"#),
        ("an object whose name is empty", r#"{"name":""}"#),
    ] {
        let refused = create_template(&router, &linked, definition).await;
        assert_eq!(
            (refused.status, refused.error_type.as_str()),
            (400, INVALID_PARAMETERS_ERROR_TYPE),
            "case {case}: expected 400 {INVALID_PARAMETERS_ERROR_TYPE:?}, got {} {:?} {:?}",
            refused.status,
            refused.error_type,
            refused.body
        );
    }

    // The review a submitted template waits on falls due on the clock, not on a read: the
    // status is PENDING until the stored deadline and APPROVED from it on, for every
    // template the rows above submitted.
    let state = engine.state::<Store>();
    let store = state.lock();
    let account = store
        .linked_accounts
        .get(&linked)
        .expect("the account the rows above linked");
    assert_eq!(
        account.message_templates.len(),
        3,
        "expected the three templates the rows above submitted, got {}",
        account.message_templates.len()
    );
    for (meta_template_id, template) in &account.message_templates {
        assert_eq!(
            (
                template.status_at(template.created_at),
                template.status_at(template.review_due_at)
            ),
            (TEMPLATE_STATUS_PENDING, TEMPLATE_STATUS_APPROVED),
            "case template {meta_template_id}: expected the review to fall due on the \
             clock, got {:?} at submission and {:?} at the deadline",
            template.status_at(template.created_at),
            template.status_at(template.review_due_at)
        );
    }
}

/// RULE, the success half for the business public key: the key a write sets is the key the
/// read reports, a registered number nobody set a key for reports none, and the write is
/// `@idempotent` — the second write of a key leaves the same state.
///
/// The rows differ along how the key arrives (an X.509 RSA public key PEM, at two key
/// sizes, and the ARN of a KMS key holding the key instead) and along which number holds
/// it, since the key belongs to one number.
#[tokio::test]
async fn the_business_public_key_a_write_sets_is_the_key_the_read_reports() {
    let router = emulator();
    let linked = link(
        &router,
        "waba-12121212121212121212121212121212",
        &["meta-phone-1", "meta-phone-2"],
    )
    .await;
    let uri = format!("/v1/whatsapp/waba/details?id={}", query_escape(&linked));
    let account = call(&router, "GET", &uri, None).await;
    let numbers: Vec<String> = account
        .at("/account/phoneNumbers")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["phoneNumberId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        numbers.len(),
        2,
        "setup: expected the two registered numbers, got {numbers:?}"
    );

    // A registered number nobody set a key for reports neither member.
    for number in &numbers {
        let unset = get_public_key(&router, number).await;
        assert_eq!(
            (
                unset.status,
                unset.at("/businessPublicKey").clone(),
                unset.at("/businessPublicKeySignatureStatus").clone()
            ),
            (200, Value::Null, Value::Null),
            "case {number:?} with no key set: expected a 200 reporting no key, got {} {:?}",
            unset.status,
            unset.body
        );
    }

    // (case, body for the first number, expected PEM reported back)
    let rows = vec![
        (
            "a 2048-bit key",
            json!({
                "originationPhoneNumberId": numbers[0],
                "businessPublicKey": rsa_public_key_pem(2048),
            }),
            json!(rsa_public_key_pem(2048)),
        ),
        (
            "a 3072-bit key replacing it",
            json!({
                "originationPhoneNumberId": numbers[0],
                "businessPublicKey": rsa_public_key_pem(3072),
            }),
            json!(rsa_public_key_pem(3072)),
        ),
        (
            "a key held in KMS",
            json!({
                "originationPhoneNumberId": numbers[0],
                "kmsKeyArn": kms_key_arn(),
            }),
            Value::Null,
        ),
    ];
    for (case, body, expected_pem) in rows {
        let written = put_public_key(&router, body.clone()).await;
        assert_eq!(
            written.status, 200,
            "case {case}: expected 200 from PutWhatsAppBusinessPublicKey, got {} {:?}",
            written.status, written.body
        );
        // `@idempotent`: writing it again is the same state.
        let again = put_public_key(&router, body).await;
        assert_eq!(
            again.status, 200,
            "case {case}, written again: expected 200, got {} {:?}",
            again.status, again.body
        );
        let read = get_public_key(&router, &numbers[0]).await;
        assert_eq!(
            (
                read.status,
                read.at("/businessPublicKey").clone(),
                read.string_at("/businessPublicKeySignatureStatus")
            ),
            (200, expected_pem, "VALID".to_string()),
            "case {case}: expected the key the write set, got {} {:?}",
            read.status,
            read.body
        );
        // The key belongs to ONE number: the other number still has none.
        let sibling = get_public_key(&router, &numbers[1]).await;
        assert_eq!(
            (sibling.status, sibling.at("/businessPublicKey").clone()),
            (200, Value::Null),
            "case {case}: expected the sibling number to hold no key, got {} {:?}",
            sibling.status,
            sibling.body
        );
    }

    // The two ways of naming the key are mutually exclusive, and a write that names
    // neither names no key to set.
    for (case, body) in [
        (
            "both a PEM and a KMS key",
            json!({
                "originationPhoneNumberId": numbers[0],
                "businessPublicKey": rsa_public_key_pem(2048),
                "kmsKeyArn": kms_key_arn(),
            }),
        ),
        (
            "neither a PEM nor a KMS key",
            json!({ "originationPhoneNumberId": numbers[0] }),
        ),
    ] {
        let refused = put_public_key(&router, body).await;
        assert_eq!(
            (refused.status, refused.error_type.as_str()),
            (400, INVALID_PARAMETERS_ERROR_TYPE),
            "case {case}: expected 400 {INVALID_PARAMETERS_ERROR_TYPE:?}, got {} {:?} {:?}",
            refused.status,
            refused.error_type,
            refused.body
        );
    }
}

/// RULE, the success half for media: a delete removes the object the number holds and says
/// so, and the next delete of the same id is a miss — the answer comes from the store, so
/// it changes with it.
///
/// The media objects are placed in the store directly because the operation that uploads
/// one (`PostWhatsAppMessageMedia`) is still scaffolded; the rows differ along which number
/// holds the object and how many objects that number holds.
#[tokio::test]
async fn a_delete_removes_the_object_the_number_holds_and_the_next_delete_misses() {
    let (router, engine) = emulator_with_engine();
    let first = link(&router, "waba-13131313131313131313131313131313", &["meta-phone-1"]).await;
    let second = link(&router, "waba-14141414141414141414141414141414", &["meta-phone-2"]).await;
    let first_number = first_phone_number_id(&router, &first).await;
    let second_number = first_phone_number_id(&router, &second).await;

    // Two objects on one number and one on the other, so a delete has to pick the object
    // of the number the request named.
    let held = [
        (first_number.clone(), "mediaONE"),
        (first_number.clone(), "media2two"),
        (second_number.clone(), "OTHERmedia3"),
    ];
    {
        let state = engine.state::<Store>();
        let mut store = state.lock();
        for (number, media_id) in &held {
            store
                .phone_numbers
                .get_mut(number)
                .expect("a registered number")
                .media
                .insert(
                    (*media_id).to_string(),
                    MediaObject {
                        media_id: (*media_id).to_string(),
                        stored_at: SystemTime::now(),
                    },
                );
        }
    }

    for (number, media_id) in &held {
        let deleted = delete_media(&router, media_id, number).await;
        assert_eq!(
            (deleted.status, deleted.at("/success").clone()),
            (200, json!(true)),
            "case {media_id:?} on {number:?}: expected a 200 reporting the delete, got {} {:?}",
            deleted.status,
            deleted.body
        );
        let again = delete_media(&router, media_id, number).await;
        assert_not_found(
            &format!("{media_id:?} deleted twice"),
            &again,
            ID_NOT_FOUND,
        );
    }

    // And an object of ANOTHER number: the delete is scoped to the number the request
    // named, so this is a miss rather than a delete.
    {
        let state = engine.state::<Store>();
        let mut store = state.lock();
        store
            .phone_numbers
            .get_mut(&second_number)
            .expect("a registered number")
            .media
            .insert(
                "scopedMedia".to_string(),
                MediaObject {
                    media_id: "scopedMedia".to_string(),
                    stored_at: SystemTime::now(),
                },
            );
    }
    let across = delete_media(&router, "scopedMedia", &first_number).await;
    assert_not_found(
        "another number's media object",
        &across,
        ID_NOT_FOUND,
    );
    let owned = delete_media(&router, "scopedMedia", &second_number).await;
    assert_eq!(
        (owned.status, owned.at("/success").clone()),
        (200, json!(true)),
        "case the object's own number: expected a 200 reporting the delete, got {} {:?}",
        owned.status,
        owned.body
    );
}
