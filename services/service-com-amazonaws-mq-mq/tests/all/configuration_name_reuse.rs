// SPDX-License-Identifier: CC0-1.0

//! A configuration's identity, driven over the wire.
//!
//! RULE: a configuration is identified by the `Id` the service mints for it, and its
//! caller-chosen `Name` carries only a value rule — no uniqueness constraint — so a
//! `CreateConfiguration` whose `Name` another configuration already carries is served:
//! it mints a further configuration with its own `Id`, and every reader reports each of
//! them under that shared name.
//!
//! The documented identity is `Id`'s own ("Required. The unique ID that Amazon MQ
//! generates for the configuration"), while `Name` documents only its character set and
//! length
//! (`smithy_query(query="doc", shape="CreateConfigurationRequest", member="Name")`),
//! and the model declares no uniqueness trait over it.
//!
//! The tables span what the rule generalises over: how MANY creates share one name, the
//! FORM of the shared name (one per character class the member allows, plus each
//! published length bound), the engine and strategy the sharing configurations differ
//! in, and a name a configuration this service created for a broker already carries.

use serde_json::{json, Value};
use std::collections::BTreeSet;

use service_com_amazonaws_mq_mq::ops::types::{
    AUTHENTICATION_STRATEGY_VALUES, ENGINE_TYPE_VALUES,
};
use service_com_amazonaws_mq_mq::state::{
    MAX_CONFIGURATION_NAME_LEN, MIN_CONFIGURATION_NAME_LEN,
};

use crate::broker_lifecycle::{create_body, string_at, Mq};
use crate::configuration_name::configuration_body;
use crate::configuration_wire_form::{
    authentication_strategy_on_the_wire, engine_type_on_the_wire,
};

/// Every configuration the listing reports, paged to the whole store in one call.
async fn listed_configurations(mq: &Mq) -> Vec<Value> {
    let (status, listed) = mq
        .call("GET", "/v1/configurations?maxResults=100", None)
        .await;
    assert_eq!(status, 200, "ListConfigurations: expected 200, got {listed:?}");
    listed
        .pointer("/configurations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// RULE: however many creates share one name, each is served and each mints its own id;
/// the listing then reports exactly that many configurations under the name.
#[tokio::test]
async fn creates_sharing_one_name_each_mint_their_own_configuration() {
    let mq = Mq::new("eu-west-1");

    // The counts a caller plausibly reaches by convention: a second generation, a
    // repeated apply, a per-broker template reused across a fleet.
    for shared in [2usize, 3, 5] {
        let name = format!("cfg-shared-{shared}");
        let case = format!("{shared} creates named {name:?}");
        let mut ids: Vec<String> = Vec::new();

        for attempt in 0..shared {
            let (status, code, created) = mq.create_configuration(&name).await;
            assert_eq!(
                status, 200,
                "case {case:?} attempt {attempt}: a reused name must be served, got \
                 {status} {code:?} {created:?}"
            );
            assert_eq!(
                string_at(&created, "/name"),
                name,
                "case {case:?} attempt {attempt}: expected the name sent back, got {created:?}"
            );
            let id = string_at(&created, "/id");
            assert!(
                !ids.contains(&id),
                "case {case:?} attempt {attempt}: every create must mint a distinct id, \
                 got {id:?} again among {ids:?}"
            );
            ids.push(id);
        }

        // Each id resolves to its own configuration, under the shared name.
        for id in &ids {
            let (status, code, described) = mq.describe_configuration(id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {id:?} must read back, got {status} {code:?} {described:?}"
            );
            assert_eq!(
                string_at(&described, "/id"),
                *id,
                "case {case:?}: the read must report the id asked for, got {described:?}"
            );
            assert_eq!(
                string_at(&described, "/name"),
                name,
                "case {case:?}: the stored name must be the shared one, got {described:?}"
            );
        }

        let listed = listed_configurations(&mq).await;
        let under_the_name: BTreeSet<String> = listed
            .iter()
            .filter(|entry| string_at(entry, "/name") == name)
            .map(|entry| string_at(entry, "/id"))
            .collect();
        assert_eq!(
            under_the_name,
            ids.iter().cloned().collect::<BTreeSet<String>>(),
            "case {case:?}: the listing must report every configuration under the name, \
             got {listed:?}"
        );
    }
}

/// RULE, over the FORM of the shared name: whatever a name inside the documented value
/// rule looks like, reusing it is served. One name per character class the member
/// allows, plus each published length bound.
#[tokio::test]
async fn a_name_of_any_documented_form_may_be_reused() {
    let mq = Mq::new("us-east-1");

    let names = [
        "Alphanumeric123".to_string(),
        "with-dash".to_string(),
        "with.period".to_string(),
        "with_underscore".to_string(),
        "with~tilde".to_string(),
        "aA0-._~".to_string(),
        "n".repeat(MIN_CONFIGURATION_NAME_LEN),
        "r".repeat(MAX_CONFIGURATION_NAME_LEN),
    ];

    for name in &names {
        let case = format!("name {name:?} created twice");
        let (status, code, first) = mq.create_configuration(name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: first create expected 200, got {status} {code:?} {first:?}"
        );
        let (status, code, second) = mq.create_configuration(name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the second create must be served too, got {status} {code:?} \
             {second:?}"
        );
        let (first_id, second_id) = (string_at(&first, "/id"), string_at(&second, "/id"));
        assert_ne!(
            first_id, second_id,
            "case {case:?}: the two configurations must carry distinct ids, got \
             {first_id:?} and {second_id:?}"
        );
        assert_ne!(
            string_at(&first, "/arn"),
            string_at(&second, "/arn"),
            "case {case:?}: the ARN is built around the id, so it must differ too, got \
             {second:?}"
        );
    }

    let listed = listed_configurations(&mq).await;
    for name in &names {
        let count = listed
            .iter()
            .filter(|entry| string_at(entry, "/name") == *name)
            .count();
        assert_eq!(
            count, 2,
            "case {name:?}: expected both configurations in the listing, got {count} \
             in {listed:?}"
        );
    }
}

/// RULE: the configurations sharing a name are independent records — each reports the
/// engine and strategy ITS own request set, not the first one's.
#[tokio::test]
async fn configurations_sharing_a_name_keep_their_own_members() {
    let mq = Mq::new("us-west-2");
    let name = "cfg-same-name-other-engine";
    let mut rows = 0usize;
    let mut seen: Vec<(String, String, String)> = Vec::new();

    for engine_type in ENGINE_TYPE_VALUES {
        for strategy in AUTHENTICATION_STRATEGY_VALUES {
            let case = format!("{name:?} as {engine_type}/{strategy}");
            let mut body = configuration_body(name);
            body["engineType"] = json!(engine_type);
            body["authenticationStrategy"] = json!(strategy);
            let (status, created) = mq.call("POST", "/v1/configurations", Some(body)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {created:?}"
            );
            seen.push((
                string_at(&created, "/id"),
                (*engine_type).to_string(),
                (*strategy).to_string(),
            ));
            rows += 1;
        }
    }

    for (id, engine_type, strategy) in &seen {
        let case = format!("{id:?} created as {engine_type}/{strategy}");
        let (status, code, described) = mq.describe_configuration(id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the configuration to read back, got {status} \
             {code:?} {described:?}"
        );
        assert_eq!(
            string_at(&described, "/engineType"),
            engine_type_on_the_wire(engine_type),
            "case {case:?}: each record keeps the engine its own request set, got \
             {described:?}"
        );
        assert_eq!(
            string_at(&described, "/authenticationStrategy"),
            authentication_strategy_on_the_wire(strategy),
            "case {case:?}: each record keeps the strategy its own request set, got \
             {described:?}"
        );
    }

    assert_eq!(
        rows,
        ENGINE_TYPE_VALUES.len() * AUTHENTICATION_STRATEGY_VALUES.len(),
        "every engine and strategy must share the one name, covered {rows}"
    );
}

/// RULE: the name a configuration this service created FOR a broker carries is no
/// barrier either — a caller may create a configuration of its own under that name.
#[tokio::test]
async fn a_name_a_broker_configuration_carries_may_be_created_under() {
    let mq = Mq::new("us-east-1");
    let name = "cfg-shared-with-a-broker";

    let (status, broker) = mq.create_broker(create_body(name)).await;
    assert_eq!(status, 200, "setup: expected 200, got {status} {broker:?}");

    for attempt in 0..2 {
        let (status, code, created) = mq.create_configuration(name).await;
        assert_eq!(
            status, 200,
            "attempt {attempt}: a caller's own configuration under this name must be \
             served, got {status} {code:?} {created:?}"
        );
        let id = string_at(&created, "/id");
        let (status, code, described) = mq.describe_configuration(&id).await;
        assert_eq!(
            status, 200,
            "attempt {attempt}: {id:?} must read back, got {status} {code:?} {described:?}"
        );
        assert_eq!(
            string_at(&described, "/name"),
            name,
            "attempt {attempt}: the stored name must be the one sent, got {described:?}"
        );
    }
}

/// RULE, by id: deleting one of the configurations sharing a name leaves the others
/// readable, because the delete resolves the ID it was given and nothing else.
#[tokio::test]
async fn deleting_one_of_the_configurations_sharing_a_name_leaves_the_others() {
    let mq = Mq::new("us-east-1");
    let name = "cfg-delete-one-of-three";

    let mut ids = Vec::new();
    for attempt in 0..3 {
        let (status, code, created) = mq.create_configuration(name).await;
        assert_eq!(
            status, 200,
            "setup {attempt}: expected 200, got {status} {code:?} {created:?}"
        );
        ids.push(string_at(&created, "/id"));
    }

    let deleted = ids.remove(1);
    let (status, code, answer) = mq.delete_configuration(&deleted).await;
    assert_eq!(
        status, 200,
        "deleting {deleted:?}: expected 200, got {status} {code:?} {answer:?}"
    );

    let (status, code, answer) = mq.describe_configuration(&deleted).await;
    assert_eq!(
        status, 404,
        "the deleted configuration must be gone, got {status} {code:?} {answer:?}"
    );
    for id in &ids {
        let (status, code, described) = mq.describe_configuration(id).await;
        assert_eq!(
            status, 200,
            "case {id:?}: a configuration sharing the name must survive, got {status} \
             {code:?} {described:?}"
        );
    }

    let listed = listed_configurations(&mq).await;
    let under_the_name: BTreeSet<String> = listed
        .iter()
        .filter(|entry| string_at(entry, "/name") == name)
        .map(|entry| string_at(entry, "/id"))
        .collect();
    assert_eq!(
        under_the_name,
        ids.iter().cloned().collect::<BTreeSet<String>>(),
        "the listing must report the survivors and nothing else, got {listed:?}"
    );
}
