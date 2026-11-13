// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ORIGINATION ENTITY an SMS sandbox registration needs.
//!
//! The RULE asserted here: an operation whose own work is to SEND an SMS message is
//! answered only while the account holds an origination entity able to send one. The
//! sandbox registration "sends a one-time password (OTP) to that phone number"
//! (`CreateSMSSandboxPhoneNumber` doc), so an account whose origination inventory holds
//! no SMS-capable entity is refused with the modelled `UserErrorException` (HTTP 400,
//! awsQuery code `UserError`) carrying the fixed sentence `No origination entities
//! available to send`, and NOTHING is stored: no pending destination number, hence no
//! one-time password, hence nothing a verification could ever complete.
//!
//! The rows differ along what the rule generalises over: the destination-number FORM
//! (both branches of the modelled pattern and both published length bounds, from the
//! sandbox suite's own catalog), the CAPABILITIES of the entities the account holds
//! (none at all, an entity that can only place a voice call or carry MMS, one that can
//! send SMS, and several together), and whether the sandbox already holds the number.
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! so nothing but persisted state carries information between calls.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_sns_amazonsimplenotificationservice::ops::types::NUMBER_CAPABILITY_VALUES;
use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    sms_origination, OriginationNumberRecord, SANDBOX_NUMBER_PENDING,
};

use super::sms_sandbox_phone_numbers::accepted_number_forms;
use super::topics_subscriptions_publish::{Answer, Sns};

/// Put one origination number with the capabilities given into the account's inventory,
/// the way the out-of-band provisioning writes it.
fn seed_origination(sns: &Sns, number: &str, capabilities: &[&str]) {
    let state = sns.state();
    let mut state = state.lock();
    state.origination_numbers.insert(
        number.to_string(),
        OriginationNumberRecord {
            phone_number: number.to_string(),
            status: "Active".to_string(),
            iso2_country_code: "US".to_string(),
            route_type: "Transactional".to_string(),
            number_capabilities: capabilities.iter().map(|value| (*value).to_string()).collect(),
            created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
        },
    );
}

/// One registration call.
async fn register(sns: &Sns, number: &str) -> Answer {
    sns.call("CreateSMSSandboxPhoneNumber", &[("PhoneNumber", number)])
        .await
}

/// Assert `answer` is the modelled origination refusal, stating AWS's sentence whole.
fn assert_refused(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        "UserError",
        "case {case:?}: expected the UserError code, got {}",
        answer.body
    );
    assert_eq!(
        answer.element("Type").as_deref(),
        Some("Sender"),
        "case {case:?}: expected a sender fault, got {}",
        answer.body
    );
    assert_eq!(
        answer.element("Message").as_deref(),
        Some(sms_origination::NO_ORIGINATION_ENTITIES),
        "case {case:?}: expected the fixed no-origination sentence, got {}",
        answer.body
    );
}

/// An account holding no origination entity cannot register a sandbox destination
/// number, whatever form that number takes — and the refusal stores nothing.
#[tokio::test]
async fn a_registration_is_refused_while_the_account_holds_no_origination_entity() {
    let sns = Sns::new();

    for (case, number) in accepted_number_forms() {
        let answer = register(&sns, &number).await;
        assert_refused(case, &answer);
        let state = sns.state();
        let state = state.lock();
        assert!(
            state.sandbox_phone_numbers.is_empty(),
            "case {case:?}: the refused registration must store nothing, got {:?}",
            state.sandbox_phone_numbers
        );
    }
}

/// The inventory is read for what the entities CAN DO: only an entity able to send SMS
/// lets a registration through.
#[tokio::test]
async fn only_an_sms_capable_origination_entity_lets_a_registration_through() {
    // (case, the capabilities the account's entities hold, whether a registration is
    // served). The capability names are the values the modelled `NumberCapability` enum
    // accepts (`crate::ops::types::NUMBER_CAPABILITY_VALUES`).
    let rows: &[(&str, &[&[&str]], bool)] = &[
        ("no entity at all", &[], false),
        ("one voice-only entity", &[&["VOICE"]], false),
        ("one MMS-only entity", &[&["MMS"]], false),
        ("two entities, neither able to send SMS", &[&["VOICE"], &["MMS"]], false),
        ("one SMS-capable entity", &[&["SMS"]], true),
        ("one entity able to send SMS and MMS", &[&["MMS", "SMS"]], true),
        (
            "two entities, one of them able to send SMS",
            &[&["VOICE"], &["SMS"]],
            true,
        ),
        ("an entity with every modelled capability", &[NUMBER_CAPABILITY_VALUES], true),
    ];

    let destination = "+447700900123";
    for (case, inventory, served) in rows {
        let sns = Sns::new();
        for (at, capabilities) in inventory.iter().enumerate() {
            seed_origination(&sns, &format!("+1206555{at:04}"), capabilities);
        }

        let answer = register(&sns, destination).await;
        if !*served {
            assert_refused(case, &answer);
            assert!(
                sns.state().lock().sandbox_phone_numbers.is_empty(),
                "case {case:?}: the refused registration must store nothing"
            );
            continue;
        }

        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the registration to be served, got {} / {}",
            answer.status, answer.body
        );
        let state = sns.state();
        let state = state.lock();
        let record = state
            .sandbox_phone_numbers
            .get(destination)
            .unwrap_or_else(|| panic!("case {case:?}: the served registration must store a record"));
        assert_eq!(
            record.status, SANDBOX_NUMBER_PENDING,
            "case {case:?}: expected a pending destination number, got {record:?}"
        );
        assert!(
            !record.one_time_password.is_empty(),
            "case {case:?}: the served registration must deliver a one-time password"
        );
    }
}

/// The pre-flight is asked on EVERY registration, the repeat of one the sandbox already
/// holds included: an inventory that is emptied out of band stops the next registration
/// and leaves the record the earlier one wrote exactly as it was.
#[tokio::test]
async fn a_registration_is_refused_once_the_inventory_no_longer_holds_an_sms_entity() {
    let sns = Sns::new();
    let origination = "+12065550199";
    let destination = "+12065550100";
    seed_origination(&sns, origination, &["SMS"]);

    let served = register(&sns, destination).await;
    assert_eq!(
        served.status, 200,
        "the first registration must be served, got {} / {}",
        served.status, served.body
    );
    let delivered = sns.state().lock().sandbox_phone_numbers[destination]
        .one_time_password
        .clone();

    sns.state().lock().origination_numbers.remove(origination);

    for (case, number) in [
        ("a number the sandbox already holds", destination),
        ("a number the sandbox does not hold", "+447700900123"),
    ] {
        let answer = register(&sns, number).await;
        assert_refused(case, &answer);
    }

    let state = sns.state();
    let state = state.lock();
    assert_eq!(
        state.sandbox_phone_numbers.len(),
        1,
        "the refused registrations must not add a record, got {:?}",
        state.sandbox_phone_numbers
    );
    assert_eq!(
        state.sandbox_phone_numbers[destination].one_time_password, delivered,
        "a refused registration must not supersede the delivered one-time password"
    );
}
