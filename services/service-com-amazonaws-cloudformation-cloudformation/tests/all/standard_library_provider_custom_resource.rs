// SPDX-License-Identifier: CC0-1.0

//! A custom resource whose provider imports the node STANDARD LIBRARY in its module body —
//! the shape every provider a CDK custom resource deploys has — reaches a terminal state on
//! that provider's own answer.
//!
//! The rule under test: **a provider function's module body is evaluated before its handler is
//! ever entered, so a provider whose body imports a module of its runtime's standard library
//! has to load for the request to be answered at all. A managed node runtime promises that
//! library: every module of it imports, only the CALL that needs a host capability the runtime
//! has none of is refused, and the console the library installs carries the whole surface a
//! program calls on it — including the `Console` a provider builds over the process's own
//! output stream to log with.**
//!
//! That is what the no-response sentence is the absence of: a module body that cannot be
//! imported never runs, so nothing is PUT to the request's `ResponseURL` and CloudFormation
//! fails the resource with "CloudFormation did not receive a response from your Custom
//! Resource" — which this suite asserts is nowhere in the stack's own events, on the create
//! and on the delete alike.
//!
//! Every row drives the emulator through its own wire: the stack operations over
//! CloudFormation's awsQuery endpoint, the provider created in AWS Lambda by the stack itself,
//! and the answer sent by the provider's own code — whose `Data` the stack's outputs then
//! carry, so a row whose provider answered without running its own code fails here.
//!
//! The rows differ along what the rule generalises over: WHICH module of the standard library
//! the provider's module body imports and what it is for — the console the library installs
//! (whose `Console` the provider logs through), and a module whose whole capability this
//! runtime has none of, where the import still has to load and the refusal belongs to the
//! call.
//!
//! The sibling suite [`super::bundled_provider_custom_resource`] pins the same rule for the
//! SDK installer's own entry point; what only this suite reaches is the console and the
//! modules beside it.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, create_stack, member, router_with_engine, settled_stack, xml_text, xml_texts, REGION,
};

/// The sentence CloudFormation words a resource's failure with when no provider answer ever
/// arrives — the fixed text, whose one variable part is the request id. No row may produce it.
const NO_RESPONSE: &str = "CloudFormation did not receive a response from your Custom Resource.";

/// The provider every row deploys: its module body imports the module the row names, builds a
/// console of its own over the process's output stream — the logger a bundled provider logs
/// through — and answers the request with what its own code read off that module.
///
/// `__IMPORTS__` is the row's own module body; `__REPORT__` is the expression whose value the
/// answer carries as `Data.Imported`.
const PROVIDER_SOURCE: &str = r#"
'use strict';

const { Console } = require('console');
const https = require('https');
const url = require('url');
__IMPORTS__

// The logger a bundled provider builds: a console of its own over the process's own output
// stream, constructed in the module body.
const logger = new Console({ stdout: process.stdout, stderr: process.stderr });

// The response document, PUT to the address the request carried.
function respond(event, status, reason, physicalResourceId, data) {
    const document = JSON.stringify({
        Status: status,
        Reason: reason,
        PhysicalResourceId: physicalResourceId,
        StackId: event.StackId,
        RequestId: event.RequestId,
        LogicalResourceId: event.LogicalResourceId,
        NoEcho: false,
        Data: data,
    });
    const parsed = url.parse(event.ResponseURL);
    return new Promise((resolve, reject) => {
        try {
            const request = https.request({
                hostname: parsed.hostname,
                path: parsed.path,
                method: 'PUT',
                headers: {
                    'content-type': '',
                    'content-length': Buffer.byteLength(document, 'utf8'),
                },
            }, () => resolve());
            request.on('error', reject);
            request.write(document);
            request.end();
        } catch (error) {
            reject(error);
        }
    });
}

exports.handler = async (event) => {
    const physicalResourceId = event.PhysicalResourceId || 'imported-standard-library';
    let status = 'SUCCESS';
    let reason = 'OK';
    let data = {};
    try {
        logger.group('the provider request');
        logger.log('%s %s', event.RequestType, event.LogicalResourceId);
        logger.groupEnd();
        data = { Imported: String(__REPORT__), RequestType: event.RequestType };
    } catch (error) {
        status = 'FAILED';
        reason = error.message;
    }
    await respond(event, status, reason, physicalResourceId, data);
    return {};
};
"#;

/// One row: the module the provider's body imports, what the provider reports off it, and the
/// stack it is deployed in.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    /// The module body's own imports, beside the ones every provider has.
    imports: &'static str,
    /// The expression whose value the provider answers as `Data.Imported`.
    report: &'static str,
    /// What that value must be.
    imported: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        // The console the standard library installs: the provider logs through a `Console` of
        // its own, built in the module body, and reports what the module answered.
        Row {
            case: "a provider logging through a console it built in its module body",
            stack_name: "standard-library-console-provider",
            imports: "",
            report: "require('console') === console",
            imported: "true",
        },
        // A module whose whole capability this runtime has none of: the import loads, the
        // computation is performed, and the call that needs the capability is refused — which
        // the provider handles, exactly as its own code says it does.
        Row {
            case: "a provider whose body imports the modules of an absent capability",
            stack_name: "standard-library-absent-provider",
            imports: "const tty = require('tty');\nconst dgram = require('dgram');",
            report: "tty.isatty(1) === false && typeof dgram.createSocket === 'function'",
            imported: "true",
        },
    ]
}

/// The provider source one row deploys.
fn provider_source(row: &Row) -> String {
    PROVIDER_SOURCE
        .replace("__IMPORTS__", row.imports)
        .replace("__REPORT__", row.report)
}

/// The template one row deploys: the execution role the provider assumes, the provider itself,
/// and the custom resource that provider backs, whose answer the outputs then carry.
fn template(row: &Row) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Role": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Action": "sts:AssumeRole",
                            "Effect": "Allow",
                            "Principal": {"Service": "lambda.amazonaws.com"},
                        }],
                    },
                },
            },
            "Provider": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "Code": {"ZipFile": provider_source(row)},
                    "Handler": "index.handler",
                    "Role": {"Fn::GetAtt": ["Role", "Arn"]},
                    "Runtime": "nodejs22.x",
                    "Timeout": 60,
                },
            },
            "Imported": {
                "Type": "Custom::StandardLibrary",
                "Properties": {
                    "ServiceToken": {"Fn::GetAtt": ["Provider", "Arn"]},
                },
            },
        },
        "Outputs": {
            "ResourceRef": {"Value": {"Ref": "Imported"}},
            "Imported": {"Value": {"Fn::GetAtt": ["Imported", "Imported"]}},
            "RequestType": {"Value": {"Fn::GetAtt": ["Imported", "RequestType"]}},
        },
    })
    .to_string()
}

/// A stack's reported outputs, as the key/value map a client reads.
fn outputs_of(xml: &str) -> std::collections::BTreeMap<String, String> {
    xml_texts(xml, "OutputKey")
        .into_iter()
        .zip(xml_texts(xml, "OutputValue"))
        .collect()
}

/// Every `ResourceStatusReason` the stack's own events carry, so a row can assert that the
/// no-response sentence is not among them.
async fn event_reasons(router: &MultiServiceRouter, stack_name: &str) -> Vec<String> {
    let (status_code, body) =
        call(router, "DescribeStackEvents", &[member("StackName", stack_name)]).await;
    assert_eq!(
        status_code, 200,
        "expected DescribeStackEvents to answer for {stack_name:?}, got {status_code} with {body}"
    );
    xml_texts(&body, "ResourceStatusReason")
}

/// A provider whose module body imports the standard library answers its request, so the
/// resource settles on that answer — and the delete is answered the same way.
#[tokio::test]
async fn a_provider_importing_the_standard_library_answers_its_request() {
    for row in rows() {
        let case = row.case;
        let (router, _engine) = router_with_engine();
        let (status_code, created) = create_stack(
            &router,
            row.stack_name,
            &template(&row),
            &[member("Capabilities.member.1", "CAPABILITY_NAMED_IAM")],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status_code} with {created}"
        );
        let settled = settled_stack(case, &router, REGION, row.stack_name).await;
        let reasons = event_reasons(&router, row.stack_name).await;
        assert!(
            !reasons.iter().any(|reason| reason.contains(NO_RESPONSE)),
            "case {case:?}: no event may report the no-response sentence, got {reasons:?}"
        );
        assert_eq!(
            xml_text(&settled, "StackStatus").unwrap_or_default(),
            "CREATE_COMPLETE",
            "case {case:?}: expected the stack to complete on its provider's answer, got \
             {settled} with {reasons:?}"
        );
        // The provider's answer IS the resource: the id it reported is what `Ref` resolves to
        // and the `Data` its own code computed is what `Fn::GetAtt` resolves against.
        let outputs = outputs_of(&settled);
        assert_eq!(
            outputs.get("ResourceRef").map(String::as_str),
            Some("imported-standard-library"),
            "case {case:?}: expected Ref to resolve to the answered id, got {outputs:?}"
        );
        assert_eq!(
            outputs.get("Imported").map(String::as_str),
            Some(row.imported),
            "case {case:?}: expected the provider to have read {:?} off the module it \
             imported, got {outputs:?}",
            row.imported
        );
        assert_eq!(
            outputs.get("RequestType").map(String::as_str),
            Some("Create"),
            "case {case:?}: expected the provider to have answered the Create request, got \
             {outputs:?}"
        );

        // ...and the DELETE is answered too, so the stack a client stood up comes down: the
        // provider's module body is imported again and the request answered the same way.
        let (deleted_code, deleted) =
            call(&router, "DeleteStack", &[member("StackName", row.stack_name)]).await;
        assert_eq!(
            deleted_code, 200,
            "case {case:?}: expected DeleteStack to be accepted, got {deleted_code} with {deleted}"
        );
        let after = settled_stack(case, &router, REGION, row.stack_name).await;
        let status_after = if after.is_empty() {
            // A completed deletion is a stack this service no longer reports by name.
            "DELETE_COMPLETE".to_string()
        } else {
            xml_text(&after, "StackStatus").unwrap_or_default()
        };
        assert_eq!(
            status_after, "DELETE_COMPLETE",
            "case {case:?}: expected the delete to settle on the provider's answer, got \
             {status_after:?} with {after}"
        );
    }
}
