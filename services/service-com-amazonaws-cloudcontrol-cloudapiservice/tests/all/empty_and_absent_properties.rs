// SPDX-License-Identifier: CC0-1.0

//! The property a resource CARRIES, as Cloud Control API reports it: exactly what the
//! OWNING service reports, with no empty value invented for a property the resource holds
//! none of.
//!
//! RULE: a resource model carries the properties the resource itself carries, read off the
//! owning service's own answer — so a property that service reports no value for is absent
//! from the model rather than reported as an empty structure, and the two reads agree on
//! that property in every state. A client that reads the model back to decide what to
//! change (a drift read, a patch built from the reported model) is then reading the owning
//! service's facts rather than this service's rendering of them.
//!
//! `AWS::Lambda::Function` is the type driven here, over the property whose value is itself a
//! structure of one map (`Environment`, whose `Variables` is "environment variables that are
//! accessible from function code during execution"). AWS Lambda reports
//! `FunctionConfiguration$Environment` exactly while the function HOLDS at least one
//! variable — a create or an update stating an environment with no variables leaves it
//! holding none — so the model reports the property for a function carrying variables and no
//! such property for one carrying none, however the create stated it. The fact is the OWNING
//! service's — `GetFunction` is what answers the member — so each row asserts AWS Lambda's
//! own read beside the Cloud Control model, and the rows are created through BOTH write paths
//! (this service's `CreateResource` and AWS Lambda's own `CreateFunction`), because the model
//! is the function's own however it was created.
//!
//! The rows differ along what the rule generalises over: how the create states the property
//! (absent, present but carrying no member, present carrying an empty map, present carrying
//! variables) and which API performed the create. One further row drives the property from
//! absent to filled and back through the owning service's own update, because what the model
//! reports has to follow a CHANGE rather than only a create.
//!
//! The harness is [`crate::emulator`]'s, which mounts this service and AWS Lambda on ONE
//! engine — the shape `core/router/src/lib.rs` mounts them in — so the model is built from
//! the records AWS Lambda's own handlers wrote.

use std::io::Write;

use serde_json::{json, Value};

use crate::emulator::{properties_of, text, Emulator};

/// The type under test.
const TYPE_NAME: &str = "AWS::Lambda::Function";

/// The property that carries the type's primary identifier.
const FUNCTION_NAME: &str = "FunctionName";

/// The property under test, and the member its structure carries.
const ENVIRONMENT: &str = "Environment";
const VARIABLES: &str = "Variables";

/// The role a function created here runs as: AWS Lambda serves a create naming a role no IAM
/// record is held for, so nothing has to be seeded.
const ROLE: &str = "arn:aws:iam::000000000000:role/a-function-role";

/// The runtime and handler every row's package is loaded through.
const RUNTIME: &str = "python3.12";
const HANDLER: &str = "index.handler";

/// The module the package carries, and its source.
const MODULE: &str = "index.py";
const SOURCE: &str = "def handler(event, context):\n    return {}\n";

/// One deployment package: the source as the only file of a .zip archive, base64-encoded as
/// a JSON request carries a blob member. Built with the maintained implementation of the
/// format, entirely in memory.
fn package() -> String {
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    archive
        .start_file(MODULE, options)
        .expect("the archive takes an entry");
    archive
        .write_all(SOURCE.as_bytes())
        .expect("the entry takes the source");
    let bytes = archive.finish().expect("the archive finishes").into_inner();
    aws_smithy_types::base64::encode(&bytes)
}

/// The model of a function named `name`, carrying `environment` as its `Environment`
/// property where the row states one.
fn model(name: &str, environment: Option<Value>) -> Value {
    let mut model = json!({
        FUNCTION_NAME: name,
        "Role": ROLE,
        "Runtime": RUNTIME,
        "Handler": HANDLER,
        "Code": { "ZipFile": package() },
    });
    if let Some(environment) = environment {
        model[ENVIRONMENT] = environment;
    }
    model
}

/// Which API created the function a row reads back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wrote {
    /// This service's own `CreateResource`.
    CloudControl,
    /// AWS Lambda's own `CreateFunction`, over the same model's members.
    OwningService,
}

/// What AWS Lambda's own read reports as the function's `Environment`, or `None` when it
/// reports no such member.
async fn lambda_environment(emulator: &Emulator, name: &str) -> Option<Value> {
    let (status, body) = emulator
        .lambda("GET", &format!("/2015-03-31/functions/{name}"), None)
        .await;
    assert_eq!(
        status, 200,
        "function {name:?}: expected AWS Lambda's own read to report it, got {status} with \
         body {body:?}"
    );
    body["Configuration"].get(ENVIRONMENT).cloned()
}

/// Create the function `model` describes through `wrote`, and answer the Cloud Control model
/// a read of it reports.
async fn created_and_read(emulator: &Emulator, wrote: Wrote, model: &Value) -> Value {
    let name = text(model, FUNCTION_NAME);
    match wrote {
        Wrote::CloudControl => {
            let identifier = emulator.provision(TYPE_NAME, model).await;
            assert_eq!(
                identifier, name,
                "expected the resource to be provisioned under the name its model states"
            );
        }
        Wrote::OwningService => {
            emulator
                .lambda_ok("POST", "/2015-03-31/functions", Some(model))
                .await;
        }
    }
    let fetched = emulator.get(TYPE_NAME, &name).await;
    assert_eq!(
        fetched.0, 200,
        "GetResource {name:?}: expected HTTP 200, got {} with body {:?}",
        fetched.0, fetched.1
    );
    properties_of(&fetched.1["ResourceDescription"])
}

#[tokio::test]
async fn a_property_the_owning_service_reports_is_in_the_model_and_one_it_omits_is_not() {
    // Each row: how the create states `Environment`, and what both reads must then report
    // for it — `None` meaning the property is part of no model and no API answer. An
    // environment stating no variable leaves the function holding none, which is the same
    // function as one created with no environment at all.
    let rows: [(&str, Option<Value>, Option<Value>); 4] = [
        ("no Environment property at all", None, None),
        ("an Environment carrying no member", Some(json!({})), None),
        (
            "an Environment carrying an empty Variables map",
            Some(json!({ VARIABLES: {} })),
            None,
        ),
        (
            "an Environment carrying variables",
            Some(json!({ VARIABLES: { "STAGE": "beta" } })),
            Some(json!({ VARIABLES: { "STAGE": "beta" } })),
        ),
    ];

    for (index, (case, stated, expected)) in rows.into_iter().enumerate() {
        for wrote in [Wrote::CloudControl, Wrote::OwningService] {
            let emulator = Emulator::with_lambda();
            let name = format!("empty-vs-absent-{index}");
            let properties =
                created_and_read(&emulator, wrote, &model(&name, stated.clone())).await;
            assert_eq!(
                properties.get(ENVIRONMENT),
                expected.as_ref(),
                "case {case:?} written through {wrote:?}: expected the model to report \
                 {ENVIRONMENT} as {expected:?}, got {:?} in {properties:?}",
                properties.get(ENVIRONMENT)
            );
            // The fact is the owning service's: the property is reported for the
            // environment the FUNCTION carries, which is what its own read answers — the
            // same value, down to its absence.
            let reported = lambda_environment(&emulator, &name).await;
            assert_eq!(
                reported.as_ref(),
                expected.as_ref(),
                "case {case:?} written through {wrote:?}: expected AWS Lambda's own read to \
                 report {ENVIRONMENT} as {expected:?}, got {reported:?}"
            );
        }
    }
}

/// How long a wait for AWS Lambda to report a function out of the window it provisions one
/// in runs before the case fails, and how often that wait re-reads: a span the clock owes, so
/// the case waits for the STATE it needs rather than for a span of its own.
const ACTIVE_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);
const ACTIVE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);

/// Wait until AWS Lambda reports `name` ready for another modification: `Active`, with no
/// update of its own still in progress.
async fn await_active(emulator: &Emulator, name: &str) {
    let started = std::time::Instant::now();
    loop {
        let (status, body) = emulator
            .lambda("GET", &format!("/2015-03-31/functions/{name}"), None)
            .await;
        let configuration = &body["Configuration"];
        let state = text(configuration, "State");
        let update = text(configuration, "LastUpdateStatus");
        if status == 200 && state == "Active" && update != "InProgress" {
            return;
        }
        assert!(
            started.elapsed() < ACTIVE_BUDGET,
            "function {name:?}: expected AWS Lambda to report it Active and settled within \
             {ACTIVE_BUDGET:?}, it is still {state:?}/{update:?} (HTTP {status}, body \
             {body:?})"
        );
        tokio::time::sleep(ACTIVE_INTERVAL).await;
    }
}

#[tokio::test]
async fn the_model_follows_every_change_of_the_property() {
    // The property is driven through each of its states by the owning service's own update,
    // so what the model reports is what the function carries NOW rather than what its create
    // stated: nothing, nothing again (an environment stating no variable holds none),
    // variables, and nothing once the update empties them.
    let emulator = Emulator::with_lambda();
    let name = "environment-changes-fn";
    let properties = created_and_read(
        &emulator,
        Wrote::OwningService,
        &model(name, None),
    )
    .await;
    assert_eq!(
        properties.get(ENVIRONMENT),
        None,
        "a function created with no {ENVIRONMENT} carries none, got {:?}",
        properties.get(ENVIRONMENT)
    );

    for (case, applied, expected) in [
        (
            "an update stating an environment with no variables",
            json!({ VARIABLES: {} }),
            None,
        ),
        (
            "an update stating variables",
            json!({ VARIABLES: { "STAGE": "beta" } }),
            Some(json!({ VARIABLES: { "STAGE": "beta" } })),
        ),
        (
            "an update emptying them again",
            json!({ VARIABLES: {} }),
            None,
        ),
    ] {
        await_active(&emulator, name).await;
        emulator
            .lambda_ok(
                "PUT",
                &format!("/2015-03-31/functions/{name}/configuration"),
                Some(&json!({ ENVIRONMENT: applied })),
            )
            .await;
        let properties = emulator.properties(TYPE_NAME, name).await;
        assert_eq!(
            properties.get(ENVIRONMENT),
            expected.as_ref(),
            "case {case:?}: expected the model to report {ENVIRONMENT} as {expected:?}, got \
             {:?}",
            properties.get(ENVIRONMENT)
        );
    }
}
