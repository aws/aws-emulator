// SPDX-License-Identifier: CC0-1.0

//! EVERY read of a VPC-attached function's configuration reports the dual-stack egress
//! setting of its attachment.
//!
//! RULE: "Allows outbound IPv6 traffic on VPC functions that are connected to dual-stack
//! subnets" (`VpcConfigResponse$Ipv6AllowedForDualStack`, a `NullableBoolean`) is a SETTING
//! of the attachment, which every attachment therefore has — off on one whose request did
//! not turn it on. A reported `VpcConfigResponse` carries the value the stored configuration
//! holds, so a client reads the setting unconditionally rather than inferring it from a
//! missing member; and because the setting belongs to the configuration rather than to any
//! one call, it is reported the same by every operation that reports a function
//! configuration.
//!
//! The suite over how a create or an update SPELLS the setting is
//! `tests/all/vpc_attachment_reported_settings.rs`; this one spans the OTHER axis the rule
//! generalises over — the READER. Its rows are every operation of this service that reports
//! a `FunctionConfiguration` for a function (`CreateFunction`, `GetFunctionConfiguration`
//! unqualified and at a published version, `GetFunction`, `ListFunctions`,
//! `ListVersionsByFunction`, `PublishVersion` and `UpdateFunctionCode`), crossed with how
//! the create spelled the setting (not at all, off, on).
//!
//! The VPC and its subnet are MINTED by Amazon EC2 itself, on the one shared engine the
//! router mounts both services on, so the attachment every row reads back is one the account
//! really holds. Nothing here sleeps: the settle the clock owes a write is brought forward.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_function, json_call, make_activation_due, make_update_due, package_bytes, zip_file,
    Answer, ROLE,
};
use crate::vpc_attachment_interfaces::{emulator, network};

/// Where the function operations of this suite are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The member every row reads, under the reported networking configuration.
const SETTING_POINTER: &str = "/VpcConfig/Ipv6AllowedForDualStack";

/// `CreateFunction` attaching `name` to `subnet_ids` with `group_ids`, spelling the
/// dual-stack setting as `setting` says — or leaving it out of the request entirely.
async fn create_attached(
    router: &MultiServiceRouter,
    name: &str,
    subnet_ids: &[String],
    group_ids: &[String],
    setting: Option<bool>,
) -> Answer {
    let mut vpc_config = json!({ "SubnetIds": subnet_ids, "SecurityGroupIds": group_ids });
    if let Some(setting) = setting {
        vpc_config["Ipv6AllowedForDualStack"] = Value::Bool(setting);
    }
    let body = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Code": { "ZipFile": zip_file(&package_bytes(64)) },
        "VpcConfig": vpc_config,
    });
    create_function(router, &body).await
}

/// Dispatch one call, asserting it answered `expected`, and answer its JSON body.
async fn ok_json(
    router: &MultiServiceRouter,
    case: &str,
    reader: &str,
    method: &str,
    uri: &str,
    body: &Value,
    expected: u16,
) -> Value {
    let answer = json_call(router, method, uri, body).await;
    assert_eq!(
        answer.status,
        expected,
        "case {case:?}: expected {expected} from {reader}, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer.json()
}

/// The dual-stack setting a body reports at `pointer`, or `None` when the member — or the
/// `VpcConfig` holding it — is absent, which is the omission this suite is about.
fn reported_setting(body: &Value, pointer: &str) -> Option<bool> {
    body.pointer(pointer).and_then(Value::as_bool)
}

/// The entry of a reported `FunctionConfiguration` list that names `name` at `version`.
fn listed_entry(body: &Value, member: &str, name: &str, version: &str) -> Value {
    body.get(member)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .find(|entry| {
            entry.get("FunctionName").and_then(Value::as_str) == Some(name)
                && entry.get("Version").and_then(Value::as_str) == Some(version)
        })
        .cloned()
        .unwrap_or_else(|| {
            panic!("expected {member} to hold {name} at version {version}, got {body}")
        })
}

/// RULE: every operation that reports a function's configuration reports the dual-stack
/// egress setting its attachment holds, whatever the create said about it.
#[tokio::test]
async fn every_reader_of_a_function_configuration_reports_its_dual_stack_setting() {
    let emulator = emulator();
    let net = network(&emulator.ec2, "the network every row attaches to", 44, 1).await;
    let group_ids = [net.group_id.clone()];

    // (case, what the create said about the setting, what every reader must report)
    let rows: [(&str, Option<bool>, bool); 3] = [
        ("no word about the setting", None, false),
        ("the setting spelled off", Some(false), false),
        ("the setting spelled on", Some(true), true),
    ];

    for (index, (case, asked, expected)) in rows.into_iter().enumerate() {
        let name = format!("dual-stack-readers-{index}");

        // The create's own answer is built from what it persisted, so it is the first reader.
        let created = create_attached(
            &emulator.router,
            &name,
            &net.subnet_ids,
            &group_ids,
            asked,
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: expected 201 from CreateFunction, got {} {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let mut reported = vec![(
            "CreateFunction",
            reported_setting(&created.json(), SETTING_POINTER),
        )];

        // The function settles into `Active` before it can be published or updated; the
        // instant the clock owes is brought FORWARD rather than waited out.
        make_activation_due(&emulator.engine);

        let configuration = ok_json(
            &emulator.router,
            case,
            "GetFunctionConfiguration",
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &Value::Null,
            200,
        )
        .await;
        reported.push((
            "GetFunctionConfiguration",
            reported_setting(&configuration, SETTING_POINTER),
        ));

        let function = ok_json(
            &emulator.router,
            case,
            "GetFunction",
            "GET",
            &format!("{FUNCTIONS_URI}/{name}"),
            &Value::Null,
            200,
        )
        .await;
        reported.push((
            "GetFunction",
            reported_setting(&function, &format!("/Configuration{SETTING_POINTER}")),
        ));

        let functions = ok_json(
            &emulator.router,
            case,
            "ListFunctions",
            "GET",
            FUNCTIONS_URI,
            &Value::Null,
            200,
        )
        .await;
        reported.push((
            "ListFunctions",
            reported_setting(
                &listed_entry(&functions, "Functions", &name, "$LATEST"),
                SETTING_POINTER,
            ),
        ));

        // A published version freezes the configuration, the setting with it, so the version
        // and every read addressed at it report the same value.
        let published = ok_json(
            &emulator.router,
            case,
            "PublishVersion",
            "POST",
            &format!("{FUNCTIONS_URI}/{name}/versions"),
            &json!({}),
            201,
        )
        .await;
        let version = published
            .get("Version")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: PublishVersion must report its version"))
            .to_string();
        reported.push((
            "PublishVersion",
            reported_setting(&published, SETTING_POINTER),
        ));

        let at_version = ok_json(
            &emulator.router,
            case,
            "GetFunctionConfiguration at the published version",
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration?Qualifier={version}"),
            &Value::Null,
            200,
        )
        .await;
        reported.push((
            "GetFunctionConfiguration at the published version",
            reported_setting(&at_version, SETTING_POINTER),
        ));

        let versions = ok_json(
            &emulator.router,
            case,
            "ListVersionsByFunction",
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/versions"),
            &Value::Null,
            200,
        )
        .await;
        reported.push((
            "ListVersionsByFunction",
            reported_setting(
                &listed_entry(&versions, "Versions", &name, &version),
                SETTING_POINTER,
            ),
        ));

        // A write that touches only the code leaves the attachment alone, and still reports
        // it: the setting follows the stored configuration, not the call.
        make_update_due(&emulator.engine);
        let updated = ok_json(
            &emulator.router,
            case,
            "UpdateFunctionCode",
            "PUT",
            &format!("{FUNCTIONS_URI}/{name}/code"),
            &json!({ "ZipFile": zip_file(&package_bytes(96)) }),
            200,
        )
        .await;
        reported.push((
            "UpdateFunctionCode",
            reported_setting(&updated, SETTING_POINTER),
        ));

        for (reader, actual) in reported {
            assert_eq!(
                actual,
                Some(expected),
                "case {case:?}: expected {reader} to report Ipv6AllowedForDualStack \
                 {expected}, got {actual:?}"
            );
        }
    }
}
