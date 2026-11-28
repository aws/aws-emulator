// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests that probe the other half of the node module LOADER: the `import`
//! path.
//!
//! The rule under test: a specifier an `import` cannot resolve fails the invocation as node's
//! own ES module RESOLVER reports it — the `Error` that resolver raises, naming the package or
//! the file it looked for and the module the import was written in — which is NOT the class a
//! `require` of a missing module reports (`node_loader_errors`). This suite is the boundary of
//! that rule: a module the engine's own loader resolves is reached through no require chain, so
//! the failure names no REQUIRE STACK — only a `require` carries one.
//!
//! The rows differ along the specifier's form, which is what the resolution generalises
//! over: a bare package, a scoped package, a relative file, a core module this runtime does
//! not supply, and whether the importing file is an `.mjs` or a `.js` of a package that
//! declares itself a module.

use std::io::Write as _;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::function_lifecycle::{
    create_function, make_activation_due, router_on, send, zip_file, Answer, ROLE,
};

/// Where `Invoke` is bound (`POST /2015-03-31/functions/{FunctionName}/invocations`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The class node's own module resolver raises a resolution failure under: one of the
/// language's errors, told apart from a failed `require` by its own code rather than by a
/// runtime class of Lambda's.
const RESOLVER_ERROR: &str = "Error";

/// A .zip deployment package holding `entries`.
fn package(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        for (name, source) in entries {
            writer.start_file(*name, options).expect("the entry starts");
            writer
                .write_all(source.as_bytes())
                .expect("the entry's bytes");
        }
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create a `nodejs20.x` function holding `entries`, and bring it to the state an
/// invocation needs.
async fn create_node_function(
    router: &smithy_server_runtime::MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    entries: &[(&str, &str)],
) {
    let created = create_function(
        router,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "nodejs20.x",
            "Handler": "index.handler",
            "Timeout": 10,
            "MemorySize": 256,
            "Code": { "ZipFile": zip_file(&package(entries)) },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must answer 201, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    make_activation_due(engine);
}

/// `Invoke` on `name`.
async fn invoke(router: &smithy_server_runtime::MultiServiceRouter, name: &str) -> Answer {
    send(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        "application/octet-stream",
        b"{}".to_vec(),
        &[],
    )
    .await
}

/// One row of the import table: the case, the package's entries, and the text the failure must
/// name — the package or the file node's resolver looked for.
type ImportCase<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);

#[tokio::test]
async fn a_specifier_an_import_cannot_resolve_is_the_loaders_own_failure() {
    // The case, the package, and the specifier the failure must name.
    let cases: &[ImportCase<'_>] = &[
        (
            "an .mjs importing a bare package the package does not hold",
            &[(
                "index.mjs",
                "import held from 'left-pad';\nexport const handler = async () => held;\n",
            )],
            "Cannot find package 'left-pad' imported from /var/task/index.mjs",
        ),
        (
            "an .mjs importing a scoped package no runtime supplies",
            &[(
                "index.mjs",
                "import held from '@acme/absent';\nexport const handler = async () => held;\n",
            )],
            "Cannot find package '@acme/absent' imported from /var/task/index.mjs",
        ),
        (
            "an .mjs importing a relative file the package does not hold",
            &[(
                "index.mjs",
                "import held from './absent.mjs';\nexport const handler = async () => held;\n",
            )],
            "Cannot find module '/var/task/absent.mjs' imported from /var/task/index.mjs",
        ),
        (
            "an .mjs importing a core module this runtime does not supply",
            &[(
                "index.mjs",
                "import held from 'node:absent';\nexport const handler = async () => held;\n",
            )],
            "No such built-in module: node:absent",
        ),
        (
            "a .js of a package that declares itself a module",
            &[
                ("package.json", r#"{"type":"module"}"#),
                (
                    "index.js",
                    "import held from 'left-pad';\nexport const handler = async () => held;\n",
                ),
            ],
            "Cannot find package 'left-pad' imported from /var/task/index.js",
        ),
    ];
    for (index, (case, entries, stated)) in cases.iter().enumerate() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let function = format!("node-import-{index}");
        create_node_function(&router, &engine, &function, entries).await;

        let answer = invoke(&router, &function).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: a function error is still an HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let document = answer.json();
        assert_eq!(
            document.get("errorType").and_then(Value::as_str),
            Some(RESOLVER_ERROR),
            "case {case:?}: expected the class node's module resolver raises, got {document}"
        );
        let message = document
            .get("errorMessage")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            message, *stated,
            "case {case:?}: expected {stated:?}, got {message:?}"
        );
        // Only a `require` carries a require stack: an import resolved by the engine's own
        // loader was reached through no require chain.
        assert!(
            !message.contains("Require stack"),
            "case {case:?}: an import failure names no require stack, got {message:?}"
        );
    }
}
