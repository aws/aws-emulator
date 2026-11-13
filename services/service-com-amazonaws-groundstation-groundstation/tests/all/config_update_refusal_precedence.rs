// SPDX-License-Identifier: CC0-1.0

//! Which of `UpdateConfig`'s two client refusals a caller hears first, asserted over a
//! TABLE spanning every `ConfigTypeData` member rather than one recorded call.
//!
//! One rule: a refusal that states a fact about the addressed resource's STORED state is
//! only reachable once that resource has been resolved, so an update addressed at a
//! `(configType, configId)` pair this account holds no config under is answered with the
//! missing config (HTTP 434 `ResourceNotFoundException`) whatever its `configData`
//! carries — the status a caller gets for a config that is not there cannot depend on the
//! body it happened to send.
//!
//! The rows differ along what the rule generalises over: the type the URI addresses (all
//! eight capability types), the union member the body selects (a different one per row,
//! so the mismatch is never the same pair twice), and the form of the dead pair (an id
//! that was never minted, and one whose config was created and then deleted).
//!
//! The same rule's other side is asserted too: once the addressed config IS there, a body
//! selecting another capability is the bad parameter it always was (HTTP 431), and a body
//! selecting the config's own capability is accepted and read back — so the reordering
//! above refuses nothing it used to accept.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::config_mutation_not_found_message;
use smithy_server_runtime::MultiServiceRouter;

use crate::scheduling_loop::{
    antenna_uplink_config_data, config_data_table, create_config, harness, send, Reply, ABSENT_UUID,
};

/// `PUT /config/{configType}/{configId}` — `UpdateConfig`.
///
/// `pub(crate)` so a sibling suite asserting another rule about the same write posts it
/// through one helper rather than a second copy of this body.
pub(crate) async fn update_config(
    router: &MultiServiceRouter,
    config_type: &str,
    config_id: &str,
    name: &str,
    config_data: &Value,
) -> Reply {
    send(
        router,
        "PUT",
        &format!("/config/{config_type}/{config_id}"),
        Some(json!({"name": name, "configData": config_data})),
    )
    .await
}

/// `GET /config/{configType}/{configId}` — `GetConfig`, the reader this suite proves an
/// accepted update through and a refused one against.
///
/// `pub(crate)` for the same reason [`update_config`] is.
pub(crate) async fn get_config(router: &MultiServiceRouter, config_type: &str, config_id: &str) -> Reply {
    send(
        router,
        "GET",
        &format!("/config/{config_type}/{config_id}"),
        None,
    )
    .await
}

/// `DELETE /config/{configType}/{configId}` — how the second form of a dead pair is made.
async fn delete_config(router: &MultiServiceRouter, config_type: &str, config_id: &str) -> Reply {
    send(
        router,
        "DELETE",
        &format!("/config/{config_type}/{config_id}"),
        None,
    )
    .await
}

/// The `antenna-uplink` config an `uplinkEchoConfig` row has to point at, so a row
/// carrying that member is a well-formed body rather than a dangling reference.
async fn echo_source_arn(router: &MultiServiceRouter) -> String {
    let (_, _, arn) = create_config(
        router,
        "echo source uplink",
        json!({"antennaUplinkConfig": antenna_uplink_config_data()}),
        None,
    )
    .await;
    arn
}

/// Every `(configType, configData)` pair of the model's union, paired with ANOTHER row's
/// `configData` — the body that selects a different capability than the URI's label.
///
/// Source of the eight pairs: [`config_data_table`], built from
/// `smithy_query(query="shape", shape="ConfigTypeData")` and `shape="ConfigCapabilityType"`.
/// Pairing each row with its successor (wrapping round) is what makes every row's mismatch
/// a different `(addressed type, selected type)` combination.
fn mismatch_rows(antenna_uplink_arn: &str) -> Vec<(&'static str, Value, &'static str, Value)> {
    let table = config_data_table(antenna_uplink_arn);
    (0..table.len())
        .map(|row| {
            let (config_type, own_data) = table[row].clone();
            let (other_type, other_data) = table[(row + 1) % table.len()].clone();
            (config_type, own_data, other_type, other_data)
        })
        .collect()
}

/// A pair holding no config is answered for the config that is missing, whatever union
/// member the body selects — for every capability type, over both forms of a dead pair.
#[tokio::test]
async fn an_update_of_a_pair_holding_no_config_answers_not_found_whatever_its_body_selects() {
    let router = harness();
    let antenna_uplink_arn = echo_source_arn(&router).await;

    for (config_type, own_data, other_type, other_data) in mismatch_rows(&antenna_uplink_arn) {
        // Form one: an id that was never minted. Form two: an id whose config was created
        // under this very type and then deleted — the same dead pair, reached the other
        // way round.
        let (deleted_id, _, _) = create_config(
            &router,
            &format!("doomed {config_type}"),
            own_data.clone(),
            None,
        )
        .await;
        let deleted = delete_config(&router, config_type, &deleted_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {config_type}: the config must delete so its pair is dead, got {} {}",
            deleted.status, deleted.body
        );

        for (form, config_id) in [
            ("never minted", ABSENT_UUID.to_string()),
            ("created then deleted", deleted_id.clone()),
        ] {
            let case = format!("{config_type} addressed with a {other_type} body, {form}");
            let refused = update_config(
                &router,
                config_type,
                &config_id,
                "rename",
                &other_data,
            )
            .await;
            assert_eq!(
                refused.status, 434,
                "case {case}: expected 434 for a pair holding no config, got {} {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "ResourceNotFoundException",
                "case {case}: expected ResourceNotFoundException, got {}",
                refused.body
            );
            let message = refused.string("message");
            assert_eq!(
                message,
                config_mutation_not_found_message(config_type, &config_id),
                "case {case}: the mutation path's not-found text, got {message:?}"
            );

            // And the same dead pair answers the SAME way for a body that does match the
            // label: what the caller is told about a missing config does not depend on
            // the body it sent.
            let matching = update_config(&router, config_type, &config_id, "rename", &own_data)
                .await;
            assert_eq!(
                matching.status, refused.status,
                "case {case}: a matching body must get the same status as a mismatching \
                 one, got {} vs {}",
                matching.status, refused.status
            );
            assert_eq!(
                matching.string("message"),
                message,
                "case {case}: a matching body must get the same text as a mismatching one, \
                 got {}",
                matching.body
            );
        }
    }
}

/// Once the addressed config IS there, the body is judged against it: a `configData`
/// selecting another capability is the bad parameter (431) naming `configData`, and the
/// config is left as it was; a `configData` selecting the config's own capability is
/// accepted and reads back.
#[tokio::test]
async fn an_update_of_a_live_config_judges_the_body_against_the_stored_capability() {
    let router = harness();
    let antenna_uplink_arn = echo_source_arn(&router).await;

    for (config_type, own_data, other_type, other_data) in mismatch_rows(&antenna_uplink_arn) {
        let case = format!("{config_type} addressed with a {other_type} body");
        let name = format!("live {config_type}");
        let (config_id, _, config_arn) =
            create_config(&router, &name, own_data.clone(), None).await;

        let refused = update_config(&router, config_type, &config_id, "retyped", &other_data).await;
        assert_eq!(
            refused.status, 431,
            "case {case}: expected 431 for a body of another capability, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterException",
            "case {case}: expected InvalidParameterException, got {}",
            refused.body
        );
        assert_eq!(
            refused.field("parameterName").as_str(),
            Some("configData"),
            "case {case}: the refusal must name the member at fault, got {}",
            refused.body
        );
        // The whole sentence, not a substring of it: `antenna-downlink` is a prefix of
        // `antenna-downlink-demod-decode`, so a `contains` pair would pass on a partly
        // wrong one.
        let message = refused.string("message");
        assert_eq!(
            message,
            format!(
                "configData selects config type {other_type}, but this config is of type \
                 {config_type}"
            ),
            "case {case}: the refusal names the capability the body selects and the one the \
             config is stored under, got {message:?}"
        );

        let after = get_config(&router, config_type, &config_id).await;
        assert_eq!(
            after.string("name"),
            name,
            "case {case}: a refused update must leave the config's name alone, got {}",
            after.body
        );
        assert_eq!(
            after.field("configData"),
            &own_data,
            "case {case}: a refused update must leave the config's parameters alone, got {}",
            after.field("configData")
        );

        // The accepted side of the same rule, so the refusals above are not a blanket one.
        let renamed = format!("live {config_type} renamed");
        let accepted = update_config(&router, config_type, &config_id, &renamed, &own_data).await;
        assert_eq!(
            accepted.status, 200,
            "case {case}: a body of the config's own capability must be accepted, got {} {}",
            accepted.status, accepted.body
        );
        assert_eq!(
            accepted.string("configArn"),
            config_arn,
            "case {case}: an accepted update must not move the config's ARN, got {}",
            accepted.body
        );
        let read_back = get_config(&router, config_type, &config_id).await;
        assert_eq!(
            read_back.string("name"),
            renamed,
            "case {case}: the accepted update must read back, got {}",
            read_back.body
        );
    }
}
