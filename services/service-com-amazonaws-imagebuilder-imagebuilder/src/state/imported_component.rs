// SPDX-License-Identifier: CC0-1.0

//! What an IMPORT writes: the component document a plain script is wrapped in, and the
//! comparison that decides whether an import holds the content a stored build version
//! already holds.
//!
//! `ImportComponent` "imports a component and transforms its data into a component document.
//! For the SHELL format, Image Builder wraps your script in a component document with a
//! single step that runs the script" (the operation's own documentation). So an import
//! carries a SCRIPT where a create carries a DOCUMENT, and what the component holds from
//! then on — what every later read of it reports — is the document rendered here: one phase,
//! the one the component's own `type` names, carrying one step that runs the script through
//! the action the component's `platform` runs a shell script with.
//!
//! The document is emitted by the same maintained YAML implementation this crate READS a
//! submitted document with, so a script carrying anything at all — colons, quotes, tabs,
//! blank lines — rides inside a document that parses back to exactly the script the request
//! carried, rather than one a hand-rolled quoting could tear.

use ::std::collections::BTreeMap;

use ::yaml_rust2::{yaml::Hash, Yaml};

use super::{
    ComAmazonawsImagebuilderImagebuilderState, ComponentRecord, COMPONENT_TYPE_TEST,
    PHASES_KEY, PHASE_NAME_KEY, PLATFORM_WINDOWS, TEST_PHASE_NAME,
};
use crate::ops::types::ImportComponentInput;

/// The operation an import's idempotency receipt is filed under.
///
/// An idempotency token "is spent on ONE operation"
/// ([`ComAmazonawsImagebuilderImagebuilderState::create_receipts`]), so one token carried by
/// an import and by a create names two requests, and each is answered from its own receipt.
pub const IMPORT_COMPONENT_OPERATION: &str = "ImportComponent";

/// The phase an imported component's single step runs in when the component is a `BUILD`
/// component — the counterpart of [`TEST_PHASE_NAME`], which is the phase whose lone presence
/// makes a document a `TEST` component, so the phase the wrapper writes and the type the
/// import asked for always agree.
const BUILD_PHASE_NAME: &str = "build";

/// The schema version the wrapping document declares, which is the one version of the
/// component-document schema this service reads.
const DOCUMENT_SCHEMA_VERSION: &str = "1.0";

/// The document key carrying the component's name.
const DOCUMENT_NAME_KEY: &str = "name";

/// The document key carrying the component's description, written only when the import
/// carried one.
const DOCUMENT_DESCRIPTION_KEY: &str = "description";

/// The document key carrying the schema version.
const DOCUMENT_SCHEMA_VERSION_KEY: &str = "schemaVersion";

/// The phase key carrying its steps.
const PHASE_STEPS_KEY: &str = "steps";

/// The step key naming the action module that step runs.
const STEP_ACTION_KEY: &str = "action";

/// The step key carrying that action's inputs.
const STEP_INPUTS_KEY: &str = "inputs";

/// The input key carrying the commands a shell action runs.
const STEP_COMMANDS_KEY: &str = "commands";

/// The name the wrapper gives the one step it writes: the step that runs the imported
/// script, and the only step the document declares.
const IMPORTED_STEP_NAME: &str = "ImportedShellScript";

/// The action module that runs a shell script on Linux and macOS.
const EXECUTE_BASH_ACTION: &str = "ExecuteBash";

/// The action module that runs a shell script on Windows, whose shell is PowerShell.
const EXECUTE_POWERSHELL_ACTION: &str = "ExecutePowerShell";

/// The action module the single wrapping step runs the script through on `platform`.
///
/// A SHELL script is run by the action module for that platform's own shell: PowerShell on
/// Windows, bash on the two Unix platforms this service models (`Platform` is one of
/// `Windows | Linux | macOS`).
#[must_use]
pub fn shell_action_module(platform: &str) -> &'static str {
    if platform == PLATFORM_WINDOWS {
        EXECUTE_POWERSHELL_ACTION
    } else {
        EXECUTE_BASH_ACTION
    }
}

/// The phase the single wrapping step runs in for a component of `component_type`.
///
/// "Image Builder determines the component type from the document. If the document contains a
/// single phase named test, the component type is TEST. Otherwise, the component type is
/// BUILD" — so writing the phase the requested type names is what makes the document this
/// wrapper renders report the type the import asked for.
#[must_use]
pub fn shell_phase_name(component_type: &str) -> &'static str {
    if component_type == COMPONENT_TYPE_TEST {
        TEST_PHASE_NAME
    } else {
        BUILD_PHASE_NAME
    }
}

/// The component document `script` is wrapped in, or `None` when it could not be rendered.
///
/// One phase — [`shell_phase_name`] of the requested type — carrying one step named
/// [`IMPORTED_STEP_NAME`] that runs the whole script through [`shell_action_module`] of the
/// requested platform. `description` is written only when the import carried one, since a
/// document key carrying nothing is not a description.
#[must_use]
pub fn shell_component_document(
    name: &str,
    description: Option<&str>,
    platform: &str,
    component_type: &str,
    script: &str,
) -> Option<String> {
    let mut inputs = Hash::new();
    inputs.insert(
        key(STEP_COMMANDS_KEY),
        Yaml::Array(vec![Yaml::String(script.to_string())]),
    );

    let mut step = Hash::new();
    step.insert(key(PHASE_NAME_KEY), key(IMPORTED_STEP_NAME));
    step.insert(key(STEP_ACTION_KEY), key(shell_action_module(platform)));
    step.insert(key(STEP_INPUTS_KEY), Yaml::Hash(inputs));

    let mut phase = Hash::new();
    phase.insert(key(PHASE_NAME_KEY), key(shell_phase_name(component_type)));
    phase.insert(key(PHASE_STEPS_KEY), Yaml::Array(vec![Yaml::Hash(step)]));

    let mut document = Hash::new();
    document.insert(key(DOCUMENT_NAME_KEY), Yaml::String(name.to_string()));
    if let Some(description) = description {
        document.insert(
            key(DOCUMENT_DESCRIPTION_KEY),
            Yaml::String(description.to_string()),
        );
    }
    // The schema version is a document VERSION rather than text, which is how a component
    // document states it (`schemaVersion: 1.0`).
    document.insert(
        key(DOCUMENT_SCHEMA_VERSION_KEY),
        Yaml::Real(DOCUMENT_SCHEMA_VERSION.to_string()),
    );
    document.insert(key(PHASES_KEY), Yaml::Array(vec![Yaml::Hash(phase)]));

    let mut rendered = String::new();
    ::yaml_rust2::YamlEmitter::new(&mut rendered)
        .dump(&Yaml::Hash(document))
        .ok()?;
    // The emitter opens what it writes with YAML's document-start marker; a component's
    // document is the document itself, which is what an inline create supplies and what a
    // read of a component reports.
    Some(
        rendered
            .strip_prefix("---\n")
            .unwrap_or(&rendered)
            .to_string(),
    )
}

/// One document, phase, step or input key as the YAML node a mapping is keyed by.
fn key(text: &str) -> Yaml {
    Yaml::String(text.to_string())
}

/// A deterministic rendering of the creation parameters one `ImportComponent` request
/// carries, which is what a retry of that token is compared against. See
/// [`super::infrastructure_configuration_request_fingerprint`] for what such a rendering
/// leaves out and why.
#[must_use]
pub fn imported_component_request_fingerprint(input: &ImportComponentInput) -> String {
    let mut fields: BTreeMap<&'static str, String> = BTreeMap::new();
    fields.insert("name", super::text(Some(input.name.as_str())));
    fields.insert(
        "semanticVersion",
        super::text(Some(input.semantic_version.as_str())),
    );
    fields.insert("type", super::text(Some(input.r#type.as_str())));
    fields.insert("format", super::text(Some(input.format.as_str())));
    fields.insert("platform", super::text(Some(input.platform.as_str())));
    fields.insert("description", super::text(input.description.as_deref()));
    fields.insert(
        "changeDescription",
        super::text(input.change_description.as_deref()),
    );
    fields.insert("data", super::text(input.data.as_deref()));
    fields.insert("uri", super::text(input.uri.as_deref()));
    fields.insert("kmsKeyId", super::text(input.kms_key_id.as_deref()));
    fields.insert("tags", super::tag_map(input.tags.as_ref()));
    fields
        .iter()
        .map(|(member, rendered)| format!("{member}={rendered};"))
        .collect()
}

/// Whether the component an import would write holds the same CONTENT as the stored build
/// version `stored`.
///
/// This is the comparison `name` documents: "If a component with the same name and semantic
/// version already exists in your account in the same Amazon Web Services Region, the request
/// creates a new build version for it. If the content is also identical to the latest build
/// version, the request fails because the component already exists." The name and the version
/// are what SELECTED `stored`, so what is compared here is everything else a component holds
/// — including the document itself, which for an import is the one the script was wrapped in,
/// so two imports of the same script collide and an import of a changed script is a new build
/// version.
#[must_use]
pub fn same_component_content(candidate: &ComponentRecord, stored: &ComponentRecord) -> bool {
    candidate.description == stored.description
        && candidate.change_description == stored.change_description
        && candidate.component_type == stored.component_type
        && candidate.platform == stored.platform
        && candidate.supported_os_versions == stored.supported_os_versions
        && candidate.data == stored.data
        && candidate.kms_key_id == stored.kms_key_id
        && candidate.tags == stored.tags
}

impl ComAmazonawsImagebuilderImagebuilderState {
    /// Store a component build version an IMPORT minted, together with the receipt for the
    /// request that minted it — filed under [`IMPORT_COMPONENT_OPERATION`], so a retry of
    /// that token is answered with the import's own original response.
    pub fn insert_imported_component(
        &mut self,
        record: ComponentRecord,
        client_token: &str,
        fingerprint: String,
        request_id: String,
    ) {
        self.file_create_receipt(
            IMPORT_COMPONENT_OPERATION,
            &record.region,
            client_token,
            fingerprint,
            request_id,
            record.arn.clone(),
        );
        self.components.insert(record.arn.clone(), record);
    }

    /// The receipt `client_token` already earned from `ImportComponent` in `request_region`,
    /// if any.
    #[must_use]
    pub fn imported_component_receipt(
        &self,
        request_region: &str,
        client_token: &str,
    ) -> Option<&super::CreateReceipt> {
        self.create_receipt(IMPORT_COMPONENT_OPERATION, request_region, client_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{parse_component_document, COMPONENT_TYPE_BUILD, PLATFORM_LINUX, PLATFORM_MACOS};

    /// Every platform this service models, with the action module its shell script runs
    /// through (`smithy_query(query="shape",shape="Platform")`: `Windows | Linux | macOS`).
    const PLATFORM_ROWS: &[(&str, &str)] = &[
        (PLATFORM_LINUX, EXECUTE_BASH_ACTION),
        (PLATFORM_MACOS, EXECUTE_BASH_ACTION),
        (PLATFORM_WINDOWS, EXECUTE_POWERSHELL_ACTION),
    ];

    /// Scripts whose text a hand-rolled quoting would tear: the rule is that the document
    /// carries the script, whatever the script holds.
    const SCRIPT_ROWS: &[(&str, &str)] = &[
        ("one line", "echo hello"),
        ("several lines", "#!/bin/bash\nset -e\necho hello\n"),
        ("a colon and a hash", "echo \"key: value\" # a comment"),
        ("quotes of both kinds", "echo 'single' \"double\""),
        ("a tab and trailing spaces", "echo\thello   \nexit 0"),
        ("a blank line between commands", "echo one\n\necho two"),
        ("a line that looks like a key", "name: not-a-document"),
        ("leading whitespace", "   echo indented"),
    ];

    /// The single step the wrapping document declares, as the YAML tree.
    fn only_step(document: &str) -> ::yaml_rust2::yaml::Hash {
        let mut loaded = ::yaml_rust2::YamlLoader::load_from_str(document)
            .unwrap_or_else(|why| panic!("the wrapped document must parse, got {why:?}"));
        assert_eq!(
            loaded.len(),
            1,
            "expected one YAML document, got {}",
            loaded.len()
        );
        let tree = loaded.remove(0);
        let phases = tree[PHASES_KEY]
            .as_vec()
            .unwrap_or_else(|| panic!("expected phases in {document}"))
            .clone();
        assert_eq!(phases.len(), 1, "expected one phase in {document}");
        let steps = phases[0][PHASE_STEPS_KEY]
            .as_vec()
            .unwrap_or_else(|| panic!("expected steps in {document}"))
            .clone();
        assert_eq!(steps.len(), 1, "expected one step in {document}");
        steps[0]
            .as_hash()
            .unwrap_or_else(|| panic!("expected a step mapping in {document}"))
            .clone()
    }

    /// RULE: the wrapping document carries the script EXACTLY, whatever the script holds, as
    /// the commands of its one step.
    #[test]
    fn the_wrapped_document_carries_the_script_exactly() {
        for (case, script) in SCRIPT_ROWS {
            let document = shell_component_document(
                "wrapped-component",
                None,
                PLATFORM_LINUX,
                COMPONENT_TYPE_BUILD,
                script,
            )
            .unwrap_or_else(|| panic!("case {case:?}: the document must render"));
            let step = only_step(&document);
            let commands = step[&key(STEP_INPUTS_KEY)][STEP_COMMANDS_KEY]
                .as_vec()
                .unwrap_or_else(|| panic!("case {case:?}: expected commands in {document}"))
                .clone();
            let carried: Vec<&str> = commands.iter().filter_map(::yaml_rust2::Yaml::as_str).collect();
            assert_eq!(
                carried,
                vec![*script],
                "case {case:?}: expected the script to ride whole, got {carried:?}"
            );
        }
    }

    /// RULE: the document the wrapper writes declares the component type the import asked
    /// for, read back through the very reader that derives a component's type from its
    /// document.
    #[test]
    fn the_wrapped_document_declares_the_requested_type() {
        for component_type in [COMPONENT_TYPE_BUILD, COMPONENT_TYPE_TEST] {
            for (platform, action) in PLATFORM_ROWS {
                let case = format!("{component_type} on {platform}");
                let document = shell_component_document(
                    "typed-component",
                    Some("a described component"),
                    platform,
                    component_type,
                    "echo hello",
                )
                .unwrap_or_else(|| panic!("case {case:?}: the document must render"));
                let parsed = parse_component_document(&document)
                    .unwrap_or_else(|why| panic!("case {case:?}: expected a document, got {why}"));
                assert_eq!(
                    parsed.component_type, component_type,
                    "case {case:?}: expected the type {component_type:?}, got {:?}",
                    parsed.component_type
                );
                let step = only_step(&document);
                let ran = step[&key(STEP_ACTION_KEY)].as_str().unwrap_or_default();
                assert_eq!(
                    ran, *action,
                    "case {case:?}: expected the action {action:?}, got {ran:?}"
                );
            }
        }
    }
}
