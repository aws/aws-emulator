// SPDX-License-Identifier: CC0-1.0

//! The WhatsApp Business Account (WABA) lifecycle, end to end over `restJson1`.
//!
//! Every case drives ONE emulator through the real router dispatch (the same path a
//! socket-served request takes) and asserts the response BODIES, so a handler that
//! answered without consulting the store would fail here.

use std::collections::BTreeSet;

use aws_smithy_types::base64;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The request path, the emulator and the signup moves every suite in this crate shares.
use crate::harness::{
    call, console_call, emulator, link, query_escape, setup_body, start_signup, Answer, ACCOUNT_ID,
    INVALID_PARAMETERS_ERROR_TYPE,
};

// Every sign-up call goes through the console flow this operation is served through; the
// refusal of one that does not is `console_signup_admission`'s own suite.
async fn associate(router: &MultiServiceRouter, body: Value) -> Answer {
    console_call(router, "POST", "/v1/whatsapp/signup", Some(body)).await
}

async fn get_account(router: &MultiServiceRouter, id: &str) -> Answer {
    let uri = format!("/v1/whatsapp/waba/details?id={}", query_escape(id));
    call(router, "GET", &uri, None).await
}

async fn get_phone_number(router: &MultiServiceRouter, id: &str) -> Answer {
    let uri = format!("/v1/whatsapp/waba/phone/details?id={}", query_escape(id));
    call(router, "GET", &uri, None).await
}

async fn list_accounts(router: &MultiServiceRouter, query: &str) -> Answer {
    let uri = format!("/v1/whatsapp/waba/list{query}");
    call(router, "GET", &uri, None).await
}

async fn send_message(router: &MultiServiceRouter, origination: &str, message: &[u8]) -> Answer {
    let body = json!({
        "originationPhoneNumberId": origination,
        // restJson1 carries a blob as base64 in the JSON body.
        "message": base64::encode(message),
        "metaApiVersion": "v20.0",
    });
    call(router, "POST", "/v1/whatsapp/send", Some(body)).await
}

async fn disassociate(router: &MultiServiceRouter, id: &str) -> Answer {
    let uri = format!("/v1/whatsapp/waba/disassociate?id={}", query_escape(id));
    call(router, "DELETE", &uri, None).await
}

async fn put_event_destinations(
    router: &MultiServiceRouter,
    id: &str,
    destinations: Value,
) -> Answer {
    let body = json!({ "id": id, "eventDestinations": destinations });
    call(
        router,
        "PUT",
        "/v1/whatsapp/waba/eventdestinations",
        Some(body),
    )
    .await
}

#[tokio::test]
async fn the_waba_lifecycle_runs_on_persisted_state() {
    let router = emulator();
    let meta_waba = "waba-01234567890123456789012345678901";
    let meta_phone = "phone-number-id-98765432109876543210987654321098";

    // 1. Associate: a signup callback hands back the token that finalizes it, then
    //    the finalization mints and stores the account.
    let callback = associate(
        &router,
        json!({ "signupCallback": { "accessToken": "meta-access-token" } }),
    )
    .await;
    assert_eq!(
        callback.status, 200,
        "signupCallback: expected 200, got {} {:?}",
        callback.status, callback.body
    );
    let token = callback.string_at("/signupCallbackResult/associateInProgressToken");
    assert!(
        !token.is_empty() && token.len() <= 50,
        "signupCallback: expected a 1..50 character associateInProgressToken, got {token:?}"
    );
    assert_eq!(
        callback.at("/linkedWhatsAppBusinessAccountId"),
        &Value::Null,
        "signupCallback alone links no account, got {:?}",
        callback.body
    );

    let finalize = associate(
        &router,
        json!({
            "setupFinalization": {
                "associateInProgressToken": token,
                "phoneNumbers": [{ "id": meta_phone, "twoFactorPin": "123456", "dataLocalizationRegion": "DE" }],
                "waba": { "id": meta_waba },
            }
        }),
    )
    .await;
    assert_eq!(
        finalize.status, 200,
        "setupFinalization: expected 200, got {} {:?}",
        finalize.status, finalize.body
    );
    let account_id = finalize.string_at("/linkedWhatsAppBusinessAccountId");
    assert!(
        account_id.starts_with("waba-"),
        "setupFinalization: expected a waba- id, got {account_id:?}"
    );
    assert_eq!(
        finalize.at("/statusCode"),
        &json!(200),
        "setupFinalization: expected statusCode 200, got {:?}",
        finalize.at("/statusCode")
    );

    // 2. List: exactly the account that was linked, with the name it was linked under.
    let listed = list_accounts(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "list: expected 200, got {} {:?}",
        listed.status, listed.body
    );
    let accounts = listed
        .at("/linkedAccounts")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        accounts.len(),
        1,
        "list: expected exactly the one linked account, got {accounts:?}"
    );
    assert_eq!(
        accounts[0]["id"].as_str(),
        Some(account_id.as_str()),
        "list: expected id {account_id:?}, got {:?}",
        accounts[0]["id"]
    );
    assert_eq!(
        accounts[0]["wabaId"].as_str(),
        Some(meta_waba),
        "list: expected wabaId {meta_waba:?}, got {:?}",
        accounts[0]["wabaId"]
    );
    assert_eq!(
        accounts[0]["wabaName"].as_str(),
        Some(meta_waba),
        "list: expected the name the account was linked under, got {:?}",
        accounts[0]["wabaName"]
    );
    assert_eq!(
        accounts[0]["registrationStatus"].as_str(),
        Some("COMPLETE"),
        "list: an account with a registered number is COMPLETE, got {:?}",
        accounts[0]["registrationStatus"]
    );
    assert!(
        accounts[0]["linkDate"].as_f64().unwrap_or_default() > 0.0,
        "list: expected the link date the association recorded, got {:?}",
        accounts[0]["linkDate"]
    );

    // 3. Read the account back: the phone number set up in step 1 is there.
    let account = get_account(&router, &account_id).await;
    assert_eq!(
        account.status, 200,
        "get: expected 200, got {} {:?}",
        account.status, account.body
    );
    let numbers = account
        .at("/account/phoneNumbers")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        numbers.len(),
        1,
        "get: expected the one number registered at association, got {numbers:?}"
    );
    assert_eq!(
        numbers[0]["metaPhoneNumberId"].as_str(),
        Some(meta_phone),
        "get: expected the Meta number id that was set up, got {:?}",
        numbers[0]["metaPhoneNumberId"]
    );
    assert_eq!(
        numbers[0]["dataLocalizationRegion"].as_str(),
        Some("DE"),
        "get: expected the data localization region that was set up, got {:?}",
        numbers[0]["dataLocalizationRegion"]
    );
    let phone_number_id = numbers[0]["phoneNumberId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        phone_number_id.starts_with("phone-number-id-"),
        "get: expected a phone-number-id- id, got {phone_number_id:?}"
    );

    // 4. Read the phone number back: it names the account that owns it.
    let phone = get_phone_number(&router, &phone_number_id).await;
    assert_eq!(
        phone.status, 200,
        "get phone: expected 200, got {} {:?}",
        phone.status, phone.body
    );
    assert_eq!(
        phone.string_at("/phoneNumber/phoneNumberId"),
        phone_number_id,
        "get phone: expected {phone_number_id:?}, got {:?}",
        phone.at("/phoneNumber/phoneNumberId")
    );
    assert_eq!(
        phone.string_at("/linkedWhatsAppBusinessAccountId"),
        account_id,
        "get phone: expected the owning account {account_id:?}, got {:?}",
        phone.at("/linkedWhatsAppBusinessAccountId")
    );

    // 5. Send: a new message id per call, and an unregistered number is not found.
    let first = send_message(&router, &phone_number_id, b"hello").await;
    let second = send_message(&router, &phone_number_id, b"hello again").await;
    assert_eq!(
        (first.status, second.status),
        (200, 200),
        "send: expected 200 twice, got {:?} and {:?}",
        first.body,
        second.body
    );
    assert_ne!(
        first.string_at("/messageId"),
        second.string_at("/messageId"),
        "send: expected a new messageId per call, got {:?} twice",
        first.at("/messageId")
    );

    let unknown_send = send_message(
        &router,
        "phone-number-id-00000000000000000000000000000000",
        b"hello",
    )
    .await;
    assert_eq!(
        (unknown_send.status, unknown_send.error_type.as_str()),
        (404, "ResourceNotFoundException"),
        "send from an unregistered number: expected 404 ResourceNotFoundException, got {} {:?} {:?}",
        unknown_send.status,
        unknown_send.error_type,
        unknown_send.body
    );

    // 6. Event destinations: what was written is what the read returns.
    let destinations = json!([{
        "eventDestinationArn": format!("arn:aws:sns:us-east-1:{ACCOUNT_ID}:social-messaging-events"),
        "roleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/social-messaging-events"),
    }]);
    let put = put_event_destinations(&router, &account_id, destinations.clone()).await;
    assert_eq!(
        put.status, 200,
        "put event destinations: expected 200, got {} {:?}",
        put.status, put.body
    );
    let after_put = get_account(&router, &account_id).await;
    assert_eq!(
        after_put.at("/account/eventDestinations"),
        &destinations,
        "put event destinations: expected the read to return what was written, got {:?}",
        after_put.at("/account/eventDestinations")
    );

    // 7. Disassociate: the account and its numbers are gone, and a second call and
    //    an id never linked are both the modelled not-found.
    let removed = disassociate(&router, &account_id).await;
    assert_eq!(
        removed.status, 200,
        "disassociate: expected 200, got {} {:?}",
        removed.status, removed.body
    );
    assert_eq!(
        removed.body,
        json!({}),
        "disassociate: expected an empty body, got {:?}",
        removed.body
    );

    let after_remove = get_account(&router, &account_id).await;
    assert_eq!(
        (after_remove.status, after_remove.error_type.as_str()),
        (404, "ResourceNotFoundException"),
        "get after disassociate: expected 404 ResourceNotFoundException, got {} {:?}",
        after_remove.status,
        after_remove.body
    );
    let phone_after_remove = get_phone_number(&router, &phone_number_id).await;
    assert_eq!(
        phone_after_remove.status, 404,
        "get phone after disassociate: expected 404, got {} {:?}",
        phone_after_remove.status, phone_after_remove.body
    );
    let listed_after = list_accounts(&router, "").await;
    assert_eq!(
        listed_after.at("/linkedAccounts"),
        &json!([]),
        "list after disassociate: expected no accounts, got {:?}",
        listed_after.at("/linkedAccounts")
    );
    let twice = disassociate(&router, &account_id).await;
    assert_eq!(
        (twice.status, twice.error_type.as_str()),
        (404, "ResourceNotFoundException"),
        "second disassociate: expected 404 ResourceNotFoundException, got {} {:?}",
        twice.status,
        twice.body
    );
}

/// One row of the association table: the shape of a setup finalization.
struct SetupCase {
    name: &'static str,
    waba_id: Option<&'static str>,
    meta_numbers: &'static [&'static str],
}

#[tokio::test]
async fn every_phone_number_a_setup_finalization_carries_becomes_one_resolvable_record() {
    // The rows differ along what the rule generalises over: how many numbers the
    // finalization carries (none, one, several), and whether it names a Meta account.
    let cases = [
        SetupCase {
            name: "no numbers at all",
            waba_id: Some("waba-11111111111111111111111111111111"),
            meta_numbers: &[],
        },
        SetupCase {
            name: "one number",
            waba_id: Some("waba-22222222222222222222222222222222"),
            meta_numbers: &["phone-number-id-aaaa"],
        },
        SetupCase {
            name: "three numbers",
            waba_id: Some("waba-33333333333333333333333333333333"),
            meta_numbers: &[
                "phone-number-id-bbbb",
                "phone-number-id-cccc",
                "phone-number-id-dddd",
            ],
        },
        SetupCase {
            name: "no waba block, so no Meta account id",
            waba_id: None,
            meta_numbers: &["phone-number-id-eeee", "phone-number-id-ffff"],
        },
    ];

    for case in cases {
        let router = emulator();
        let token = start_signup(&router).await;
        let answer = associate(
            &router,
            setup_body(&token, case.waba_id, case.meta_numbers),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.name, answer.status, answer.body
        );
        let account_id = answer.string_at("/linkedWhatsAppBusinessAccountId");
        assert!(
            account_id.starts_with("waba-") && account_id.len() == "waba-".len() + 32,
            "case {:?}: expected a waba- id with a 32 character suffix, got {account_id:?}",
            case.name
        );

        let account = get_account(&router, &account_id).await;
        assert_eq!(
            account.status, 200,
            "case {:?}: expected the minted id to read back, got {} {:?}",
            case.name, account.status, account.body
        );

        // The name the account carries is the Meta account id it was linked under;
        // with no `waba` block the minted id stands in for it.
        let expected_name = case.waba_id.unwrap_or(account_id.as_str());
        assert_eq!(
            account.string_at("/account/wabaName"),
            expected_name,
            "case {:?}: expected wabaName {expected_name:?}, got {:?}",
            case.name,
            account.at("/account/wabaName")
        );
        assert_eq!(
            account.string_at("/account/wabaId"),
            expected_name,
            "case {:?}: expected wabaId {expected_name:?}, got {:?}",
            case.name,
            account.at("/account/wabaId")
        );

        // An account with no registered number has not finished setting up.
        let expected_status = if case.meta_numbers.is_empty() {
            "INCOMPLETE"
        } else {
            "COMPLETE"
        };
        assert_eq!(
            account.string_at("/account/registrationStatus"),
            expected_status,
            "case {:?}: expected registrationStatus {expected_status:?}, got {:?}",
            case.name,
            account.at("/account/registrationStatus")
        );

        let expected_arn =
            format!("arn:aws:social-messaging:us-east-1:{ACCOUNT_ID}:waba/{}", &account_id["waba-".len()..]);
        assert_eq!(
            account.string_at("/account/arn"),
            expected_arn,
            "case {:?}: expected arn {expected_arn:?}, got {:?}",
            case.name,
            account.at("/account/arn")
        );

        let numbers = account
            .at("/account/phoneNumbers")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            numbers.len(),
            case.meta_numbers.len(),
            "case {:?}: expected one record per phoneNumbers entry, got {numbers:?}",
            case.name
        );

        let mut seen = BTreeSet::new();
        for (entry, meta_id) in numbers.iter().zip(case.meta_numbers) {
            assert_eq!(
                entry["metaPhoneNumberId"].as_str(),
                Some(*meta_id),
                "case {:?}: expected the Meta id {meta_id:?} in registration order, got {:?}",
                case.name,
                entry["metaPhoneNumberId"]
            );
            let id = entry["phoneNumberId"].as_str().unwrap_or_default();
            assert!(
                id.starts_with("phone-number-id-") && id.len() == "phone-number-id-".len() + 32,
                "case {:?}: expected a phone-number-id- id with a 32 character suffix, got {id:?}",
                case.name
            );
            assert!(
                seen.insert(id.to_string()),
                "case {:?}: expected a distinct id per registration, got {id:?} twice",
                case.name
            );
            let expected_arn = format!(
                "arn:aws:social-messaging:us-east-1:{ACCOUNT_ID}:phone-number-id/{}",
                &id["phone-number-id-".len()..]
            );
            assert_eq!(
                entry["arn"].as_str(),
                Some(expected_arn.as_str()),
                "case {:?}: expected arn {expected_arn:?}, got {:?}",
                case.name,
                entry["arn"]
            );

            // The number is in the range reserved for fictional use: +1 <area> 555-01XX.
            let phone_number = entry["phoneNumber"].as_str().unwrap_or_default();
            assert!(
                phone_number.starts_with("+1") && phone_number.contains("55501"),
                "case {:?}: expected a +1 <area> 555-01XX number, got {phone_number:?}",
                case.name
            );
            assert!(
                entry["displayPhoneNumber"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("555-01"),
                "case {:?}: expected the display form of {phone_number:?}, got {:?}",
                case.name,
                entry["displayPhoneNumber"]
            );
            assert_eq!(
                entry["displayPhoneNumberName"].as_str(),
                Some(expected_name),
                "case {:?}: expected the display name to default to the account name, got {:?}",
                case.name,
                entry["displayPhoneNumberName"]
            );

            // The record resolves through its own read, by id and by ARN alike.
            for form in [id, expected_arn.as_str()] {
                let phone = get_phone_number(&router, form).await;
                assert_eq!(
                    phone.status, 200,
                    "case {:?}: expected {form:?} to resolve, got {} {:?}",
                    case.name, phone.status, phone.body
                );
                assert_eq!(
                    phone.string_at("/phoneNumber/phoneNumberId"),
                    id,
                    "case {:?}: expected {form:?} to resolve to {id:?}, got {:?}",
                    case.name,
                    phone.at("/phoneNumber/phoneNumberId")
                );
                assert_eq!(
                    phone.string_at("/linkedWhatsAppBusinessAccountId"),
                    account_id,
                    "case {:?}: expected {form:?} to name the owning account, got {:?}",
                    case.name,
                    phone.at("/linkedWhatsAppBusinessAccountId")
                );
            }
        }
    }
}

#[tokio::test]
async fn a_list_cursor_partitions_the_linked_accounts_exactly_once() {
    // maxResults is bounded 1..100 by the model, so the rows span that range's
    // lower bound, a middle value and a page larger than the whole collection.
    for (account_count, max_results) in [(0, 1), (1, 1), (3, 1), (5, 2), (4, 4), (7, 100)] {
        let router = emulator();
        let mut expected = BTreeSet::new();
        for index in 0..account_count {
            let waba_id = format!("waba-{index:032}");
            expected.insert(link(&router, &waba_id, &["phone-number-id-x"]).await);
        }

        let case = format!("{account_count} accounts, maxResults {max_results}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= account_count + 2,
                "case {case:?}: the cursor did not terminate after {pages} pages"
            );
            let query = match token.as_deref() {
                Some(token) => format!("?maxResults={max_results}&nextToken={}", query_escape(token)),
                None => format!("?maxResults={max_results}"),
            };
            let answer = list_accounts(&router, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            let page = answer
                .at("/linkedAccounts")
                .as_array()
                .cloned()
                .unwrap_or_default();
            token = answer
                .at("/nextToken")
                .as_str()
                .map(std::string::ToString::to_string);
            if token.is_some() {
                assert_eq!(
                    page.len(),
                    max_results,
                    "case {case:?}: a page that carries a cursor must be full, got {} entries",
                    page.len()
                );
            }
            assert!(
                page.len() <= max_results,
                "case {case:?}: expected at most {max_results} entries, got {}",
                page.len()
            );
            seen.extend(
                page.iter()
                    .map(|entry| entry["id"].as_str().unwrap_or_default().to_string()),
            );
            if token.is_none() {
                break;
            }
        }

        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case:?}: the pages repeated an account, got {seen:?}"
        );
        assert_eq!(
            unique, expected,
            "case {case:?}: expected every linked account exactly once, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn an_account_id_resolves_the_same_record_in_short_form_and_in_arn_form() {
    let router = emulator();
    let account_id = link(
        &router,
        "waba-44444444444444444444444444444444",
        &["phone-number-id-gggg"],
    )
    .await;
    let short = get_account(&router, &account_id).await;
    let arn = short.string_at("/account/arn");

    for form in [account_id.as_str(), arn.as_str()] {
        let answer = get_account(&router, form).await;
        assert_eq!(
            answer.status, 200,
            "case {form:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.at("/account"),
            short.at("/account"),
            "case {form:?}: expected the same account as the short form, got {:?}",
            answer.at("/account")
        );
    }

    // The ARN form writes to, and unlinks, that same record.
    let put = put_event_destinations(&router, &arn, json!([])).await;
    assert_eq!(
        put.status, 200,
        "case ARN put: expected 200, got {} {:?}",
        put.status, put.body
    );
    let removed = disassociate(&router, &arn).await;
    assert_eq!(
        removed.status, 200,
        "case ARN disassociate: expected 200, got {} {:?}",
        removed.status, removed.body
    );
    let after = get_account(&router, &account_id).await;
    assert_eq!(
        after.status, 404,
        "case ARN disassociate: expected the short form to be gone too, got {} {:?}",
        after.status, after.body
    );
}

#[tokio::test]
async fn an_id_that_names_no_resource_is_the_modelled_not_found() {
    let router = emulator();
    // One linked account exists, so a miss is a miss on the id and not on an empty store.
    link(
        &router,
        "waba-55555555555555555555555555555555",
        &["phone-number-id-hhhh"],
    )
    .await;

    let unknown_waba = "waba-00000000000000000000000000000000";
    let unknown_waba_arn =
        format!("arn:aws:social-messaging:us-east-1:{ACCOUNT_ID}:waba/00000000000000000000000000000000");
    let unknown_phone = "phone-number-id-00000000000000000000000000000000";
    let unknown_phone_arn = format!(
        "arn:aws:social-messaging:us-east-1:{ACCOUNT_ID}:phone-number-id/00000000000000000000000000000000"
    );

    for id in [unknown_waba, unknown_waba_arn.as_str()] {
        let answer = get_account(&router, id).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case get account {id:?}: expected 404 ResourceNotFoundException, got {} {:?} {:?}",
            answer.status,
            answer.error_type,
            answer.body
        );
        let answer = disassociate(&router, id).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case disassociate {id:?}: expected 404 ResourceNotFoundException, got {} {:?} {:?}",
            answer.status,
            answer.error_type,
            answer.body
        );
        // PutWhatsAppBusinessAccountEventDestinations models no not-found, so the
        // model's own client-input error answers an id that names no account.
        let answer = put_event_destinations(&router, id, json!([])).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, INVALID_PARAMETERS_ERROR_TYPE),
            "case put event destinations {id:?}: expected 400 {INVALID_PARAMETERS_ERROR_TYPE:?}, got {} {:?} {:?}",
            answer.status,
            answer.error_type,
            answer.body
        );
    }

    for id in [unknown_phone, unknown_phone_arn.as_str()] {
        let answer = get_phone_number(&router, id).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case get phone {id:?}: expected 404 ResourceNotFoundException, got {} {:?} {:?}",
            answer.status,
            answer.error_type,
            answer.body
        );
        let answer = send_message(&router, id, b"hello").await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case send {id:?}: expected 404 ResourceNotFoundException, got {} {:?} {:?}",
            answer.status,
            answer.error_type,
            answer.body
        );
    }
}

#[tokio::test]
async fn each_send_mints_a_new_message_id() {
    let router = emulator();
    let account_id = link(
        &router,
        "waba-66666666666666666666666666666666",
        &["phone-number-id-iiii"],
    )
    .await;
    let account = get_account(&router, &account_id).await;
    let phone_number_id = account.string_at("/account/phoneNumbers/0/phoneNumberId");

    // The bodies differ in size and content, including the same body twice: the id
    // is minted per CALL, not derived from the message.
    let bodies: [&[u8]; 5] = [b"a", b"a", b"", b"a longer message body", &[0xff, 0x00, 0xfe]];
    let mut seen = BTreeSet::new();
    for (index, body) in bodies.iter().enumerate() {
        let answer = send_message(&router, &phone_number_id, body).await;
        if body.is_empty() {
            // `message` is a blob of length 1..2048000, so an empty one is refused
            // at the boundary, before the handler runs.
            assert_eq!(
                answer.status, 400,
                "case {index} (empty message): expected the modelled length bound to refuse it, got {} {:?}",
                answer.status, answer.body
            );
            continue;
        }
        assert_eq!(
            answer.status, 200,
            "case {index}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let message_id = answer.string_at("/messageId");
        assert!(
            !message_id.is_empty(),
            "case {index}: expected a minted messageId, got {message_id:?}"
        );
        assert!(
            seen.insert(message_id.clone()),
            "case {index}: expected a new messageId per call, got {message_id:?} twice"
        );
    }
    assert_eq!(
        seen.len(),
        4,
        "expected one distinct messageId per accepted send, got {seen:?}"
    );
}

#[tokio::test]
async fn a_setup_finalization_refuses_waba_together_with_phone_number_parent() {
    let router = emulator();
    let parent = link(
        &router,
        "waba-77777777777777777777777777777777",
        &["phone-number-id-jjjj"],
    )
    .await;

    // The model: `phoneNumberParent` "can't be used when the waba field is present".
    let both = associate(
        &router,
        json!({
            "setupFinalization": {
                "associateInProgressToken": start_signup(&router).await,
                "phoneNumbers": [{ "id": "phone-number-id-kkkk", "twoFactorPin": "123456" }],
                "phoneNumberParent": parent,
                "waba": { "id": "waba-88888888888888888888888888888888" },
            }
        }),
    )
    .await;
    assert_eq!(
        (both.status, both.error_type.as_str()),
        (400, INVALID_PARAMETERS_ERROR_TYPE),
        "case waba + phoneNumberParent: expected 400 {INVALID_PARAMETERS_ERROR_TYPE:?}, got {} {:?} {:?}",
        both.status,
        both.error_type,
        both.body
    );

    // A parent on its own registers the number under the account it names.
    let added = associate(
        &router,
        json!({
            "setupFinalization": {
                "associateInProgressToken": start_signup(&router).await,
                "phoneNumbers": [{ "id": "phone-number-id-llll", "twoFactorPin": "123456" }],
                "phoneNumberParent": parent,
            }
        }),
    )
    .await;
    assert_eq!(
        added.status, 200,
        "case phoneNumberParent alone: expected 200, got {} {:?}",
        added.status, added.body
    );
    assert_eq!(
        added.string_at("/linkedWhatsAppBusinessAccountId"),
        parent,
        "case phoneNumberParent alone: expected the parent account, got {:?}",
        added.at("/linkedWhatsAppBusinessAccountId")
    );
    let account = get_account(&router, &parent).await;
    let metas: Vec<&str> = account
        .at("/account/phoneNumbers")
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["metaPhoneNumberId"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        metas,
        vec!["phone-number-id-jjjj", "phone-number-id-llll"],
        "case phoneNumberParent alone: expected both numbers under the parent, got {metas:?}"
    );

    // An unknown parent is refused, not linked as a new account.
    let orphan = associate(
        &router,
        json!({
            "setupFinalization": {
                "associateInProgressToken": start_signup(&router).await,
                "phoneNumbers": [{ "id": "phone-number-id-mmmm", "twoFactorPin": "123456" }],
                "phoneNumberParent": "waba-00000000000000000000000000000000",
            }
        }),
    )
    .await;
    assert_eq!(
        (orphan.status, orphan.error_type.as_str()),
        (400, INVALID_PARAMETERS_ERROR_TYPE),
        "case unknown phoneNumberParent: expected 400 {INVALID_PARAMETERS_ERROR_TYPE:?}, got {} {:?} {:?}",
        orphan.status,
        orphan.error_type,
        orphan.body
    );
}

#[tokio::test]
async fn a_finalization_only_finalizes_a_signup_this_service_issued() {
    let router = emulator();
    let token = start_signup(&router).await;

    // Each row is a finalization the service refuses. The first quotes a token no
    // signupCallback ever issued; the rest quote the live token with some other
    // member wrong, so the refusal must leave that token still finalizable.
    let refusals = [
        (
            "a token no signupCallback issued",
            json!({
                "associateInProgressToken": "signup-00000000000000000000000000000000",
                "phoneNumbers": [{ "id": "phone-number-id-nnnn", "twoFactorPin": "123456" }],
                "waba": { "id": "waba-bcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbc" },
            }),
        ),
        (
            "unknown phoneNumberParent",
            json!({
                "associateInProgressToken": token,
                "phoneNumbers": [{ "id": "phone-number-id-oooo", "twoFactorPin": "123456" }],
                "phoneNumberParent": "waba-00000000000000000000000000000000",
            }),
        ),
        (
            "waba together with phoneNumberParent",
            json!({
                "associateInProgressToken": token,
                "phoneNumbers": [{ "id": "phone-number-id-pppp", "twoFactorPin": "123456" }],
                "phoneNumberParent": "waba-00000000000000000000000000000000",
                "waba": { "id": "waba-cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd" },
            }),
        ),
    ];
    for (case, setup) in refusals {
        let answer = associate(&router, json!({ "setupFinalization": setup })).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, INVALID_PARAMETERS_ERROR_TYPE),
            "case {case:?}: expected 400 {INVALID_PARAMETERS_ERROR_TYPE:?}, got {} {:?} {:?}",
            answer.status,
            answer.error_type,
            answer.body
        );
        let listed = list_accounts(&router, "").await;
        assert_eq!(
            listed.at("/linkedAccounts"),
            &json!([]),
            "case {case:?}: a refused finalization must link no account, got {:?}",
            listed.at("/linkedAccounts")
        );
    }

    // No refusal consumed the signup, so the caller can still finalize it.
    let finalization = json!({
        "setupFinalization": {
            "associateInProgressToken": token,
            "phoneNumbers": [{ "id": "phone-number-id-qqqq", "twoFactorPin": "123456" }],
            "waba": { "id": "waba-dededededededededededededededede" },
        }
    });
    let finalize = associate(&router, finalization.clone()).await;
    assert_eq!(
        finalize.status, 200,
        "case retry after a refusal: expected 200, got {} {:?}",
        finalize.status, finalize.body
    );
    let account_id = finalize.string_at("/linkedWhatsAppBusinessAccountId");
    assert!(
        account_id.starts_with("waba-"),
        "case retry after a refusal: expected a waba- id, got {account_id:?}"
    );

    // Finalizing consumed the signup, so the SAME request is now refused and links
    // no second account.
    let again = associate(&router, finalization).await;
    assert_eq!(
        (again.status, again.error_type.as_str()),
        (400, INVALID_PARAMETERS_ERROR_TYPE),
        "case a token already finalized: expected 400 {INVALID_PARAMETERS_ERROR_TYPE:?}, got {} {:?} {:?}",
        again.status,
        again.error_type,
        again.body
    );
    let listed = list_accounts(&router, "").await;
    assert_eq!(
        listed.at("/linkedAccounts").as_array().map(Vec::len),
        Some(1),
        "case a token already finalized: expected the one account the first call linked, got {:?}",
        listed.at("/linkedAccounts")
    );
}

#[tokio::test]
async fn a_refused_combined_request_records_no_signup() {
    use service_com_amazonaws_socialmessaging_socialmessaging::state::MAX_PENDING_SIGNUPS;

    let router = emulator();
    let callback = json!({ "signupCallback": { "accessToken": "meta-access-token" } });
    // One slot short of the signup capacity, so one further recorded signup is
    // observable: it would make the next plain callback the one past the cap.
    for index in 0..MAX_PENDING_SIGNUPS - 1 {
        let answer = associate(&router, callback.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case fill {index}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }

    // A request carrying BOTH a callback and a finalization the service refuses:
    // the refusal must leave no signup behind, since the caller never received its
    // token.
    let combined = associate(
        &router,
        json!({
            "signupCallback": { "accessToken": "meta-access-token" },
            "setupFinalization": {
                "associateInProgressToken": "signup-00000000000000000000000000000000",
                "phoneNumbers": [{ "id": "phone-number-id-rrrr", "twoFactorPin": "123456" }],
                "waba": { "id": "waba-efefefefefefefefefefefefefefefef" },
            }
        }),
    )
    .await;
    assert_eq!(
        (combined.status, combined.error_type.as_str()),
        (400, INVALID_PARAMETERS_ERROR_TYPE),
        "case callback + unissued token: expected 400 {INVALID_PARAMETERS_ERROR_TYPE:?}, got {} {:?} {:?}",
        combined.status,
        combined.error_type,
        combined.body
    );
    let listed = list_accounts(&router, "").await;
    assert_eq!(
        listed.at("/linkedAccounts"),
        &json!([]),
        "case callback + unissued token: expected no account to have been linked, got {:?}",
        listed.at("/linkedAccounts")
    );

    // The slot the refused request would have taken is still free, so this one
    // fills the capacity and only the one after it is refused.
    let last = associate(&router, callback.clone()).await;
    assert_eq!(
        last.status, 200,
        "case the slot the refusal did not take: expected 200, got {} {:?}",
        last.status, last.body
    );
    let over = associate(&router, callback).await;
    assert_eq!(
        (over.status, over.error_type.as_str()),
        (400, "LimitExceededException"),
        "case signup {} of {MAX_PENDING_SIGNUPS}: expected 400 LimitExceededException, got {} {:?} {:?}",
        MAX_PENDING_SIGNUPS + 1,
        over.status,
        over.error_type,
        over.body
    );
}

#[tokio::test]
async fn growth_past_the_store_capacity_is_refused_with_the_modelled_limit_error() {
    use service_com_amazonaws_socialmessaging_socialmessaging::state::{
        MAX_LINKED_ACCOUNTS, MAX_PENDING_SIGNUPS, MAX_PHONE_NUMBERS_PER_ACCOUNT,
    };

    // One finalization carrying more numbers than an account holds is refused
    // before anything is written, so it leaves no partly linked account behind.
    let router = emulator();
    let too_many: Vec<String> = (0..=MAX_PHONE_NUMBERS_PER_ACCOUNT)
        .map(|index| format!("phone-number-id-{index}"))
        .collect();
    let too_many: Vec<&str> = too_many.iter().map(String::as_str).collect();
    let token = start_signup(&router).await;
    let answer = associate(
        &router,
        setup_body(
            &token,
            Some("waba-99999999999999999999999999999999"),
            &too_many,
        ),
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (400, "LimitExceededException"),
        "case {} numbers in one finalization: expected 400 LimitExceededException, got {} {:?} {:?}",
        too_many.len(),
        answer.status,
        answer.error_type,
        answer.body
    );
    let listed = list_accounts(&router, "").await;
    assert_eq!(
        listed.at("/linkedAccounts"),
        &json!([]),
        "case refused finalization: expected no account to have been linked, got {:?}",
        listed.at("/linkedAccounts")
    );

    // Linking up to the capacity works; the one past it is refused.
    let router = emulator();
    for index in 0..MAX_LINKED_ACCOUNTS {
        let waba_id = format!("waba-{index:032}");
        let token = start_signup(&router).await;
        let answer = associate(
            &router,
            setup_body(
                &token,
                Some(waba_id.as_str()),
                &["phone-number-id-capacity"],
            ),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case account {index} of {MAX_LINKED_ACCOUNTS}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }
    let token = start_signup(&router).await;
    let over = associate(
        &router,
        setup_body(
            &token,
            Some("waba-ffffffffffffffffffffffffffffffff"),
            &["phone-number-id-capacity"],
        ),
    )
    .await;
    assert_eq!(
        (over.status, over.error_type.as_str()),
        (400, "LimitExceededException"),
        "case account {} of {MAX_LINKED_ACCOUNTS}: expected 400 LimitExceededException, got {} {:?} {:?}",
        MAX_LINKED_ACCOUNTS + 1,
        over.status,
        over.error_type,
        over.body
    );

    // A stream of callback-only requests is bounded the same way.
    let router = emulator();
    let callback = json!({ "signupCallback": { "accessToken": "meta-access-token" } });
    for index in 0..MAX_PENDING_SIGNUPS {
        let answer = associate(&router, callback.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case signup {index} of {MAX_PENDING_SIGNUPS}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }
    let over = associate(&router, callback).await;
    assert_eq!(
        (over.status, over.error_type.as_str()),
        (400, "LimitExceededException"),
        "case signup {} of {MAX_PENDING_SIGNUPS}: expected 400 LimitExceededException, got {} {:?} {:?}",
        MAX_PENDING_SIGNUPS + 1,
        over.status,
        over.error_type,
        over.body
    );
}

#[tokio::test]
async fn sending_past_the_retained_send_record_keeps_minting_new_message_ids() {
    use service_com_amazonaws_socialmessaging_socialmessaging::state::MAX_RETAINED_SENT_MESSAGES;

    let router = emulator();
    let account_id = link(
        &router,
        "waba-abababababababababababababababab",
        &["phone-number-id-nnnn"],
    )
    .await;
    let account = get_account(&router, &account_id).await;
    let phone_number_id = account.string_at("/account/phoneNumbers/0/phoneNumberId");

    // Trimming the record of past sends must never make a send fail or repeat an id.
    let sends = MAX_RETAINED_SENT_MESSAGES + 5;
    let mut seen = BTreeSet::new();
    for index in 0..sends {
        let answer = send_message(&router, &phone_number_id, b"retention").await;
        assert_eq!(
            answer.status, 200,
            "case send {index} of {sends}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let message_id = answer.string_at("/messageId");
        assert!(
            seen.insert(message_id.clone()),
            "case send {index} of {sends}: expected a new messageId, got {message_id:?} twice"
        );
    }
    assert_eq!(
        seen.len(),
        sends,
        "expected one distinct messageId per send, got {} for {sends} sends",
        seen.len()
    );
}
