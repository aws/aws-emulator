// SPDX-License-Identifier: CC0-1.0

//! What a revision's AppSpec DECLARES a deployment deploys.
//!
//! A deployment of the `Lambda` compute platform deploys a revision that IS the
//! AppSpec file — "the content includes a Lambda function name, the alias for its
//! original version, and the alias for its replacement version. The deployment
//! shifts traffic from the original version of the Lambda function to the
//! replacement version"
//! (`smithy_query(query="shape",shape="AppSpecContent")`). This module is the one
//! place that document is READ: what it declares is resolved once, when the
//! deployment is recorded, so no later request re-parses caller-supplied content.
//! Carrying the declared shift out is [`super::lambda_traffic_shift`]'s, and the
//! increments it is carried out in are [`super::traffic_shift`]'s.
//!
//! The document is YAML or JSON (YAML 1.2 is a superset of JSON, so one parser
//! reads both), and it is caller-supplied, so it is read through a maintained YAML
//! implementation under bounds this module states and checks BEFORE any parser
//! walks it. Nothing is inferred into the stored revision: the revision is held
//! verbatim, and this only reports what the document already says.

use ::yaml_rust2::Yaml;

use crate::ops::types::RevisionLocation;

/// The `ComputePlatform` whose deployments shift traffic between two versions of a
/// Lambda function (`crate::ops::types::COMPUTE_PLATFORM_VALUES`: ECS | Lambda |
/// Server).
pub const COMPUTE_PLATFORM_LAMBDA: &str = "Lambda";

/// The AppSpec mapping holding the resources a revision deploys.
const RESOURCES_KEY: &str = "Resources";
/// The member of one resource declaration naming what kind of resource it is.
const TYPE_KEY: &str = "Type";
/// The member of one resource declaration carrying what is deployed about it.
const PROPERTIES_KEY: &str = "Properties";
/// The resource type of the Lambda function an Lambda AppSpec deploys, which is
/// the only declaration that describes a traffic shift.
const LAMBDA_FUNCTION_RESOURCE_TYPE: &str = "AWS::Lambda::Function";
/// The property naming the function whose alias the deployment moves.
const NAME_PROPERTY: &str = "Name";
/// The property naming the alias the deployment moves.
const ALIAS_PROPERTY: &str = "Alias";
/// The property naming the version the alias invoked before the deployment.
const CURRENT_VERSION_PROPERTY: &str = "CurrentVersion";
/// The property naming the version the alias invokes after the deployment.
const TARGET_VERSION_PROPERTY: &str = "TargetVersion";
/// The AppSpec mapping holding the lifecycle event hooks a deployment runs:
/// "A deployment lifecycle hook is specified in the hooks section of the AppSpec
/// file"
/// (`smithy_query(query="doc",shape="PutLifecycleEventHookExecutionStatusInput",member="lifecycleEventHookExecutionId")`).
///
/// The key is matched without regard to case: the AppSpec reference spells the
/// section `Hooks` for the Lambda and Amazon ECS compute platforms and `hooks`
/// for EC2/on-premises, so a document is read for the section it HAS rather than
/// for one spelling of it.
const HOOKS_KEY: &str = "Hooks";

/// The longest revision content this emulator parses an AppSpec out of.
///
/// `RawStringContent` carries no modelled length
/// (`smithy_query(query="shape",shape="RawStringContent")`), and the content is
/// caller-supplied, so the bound is EMULATOR-CHOSEN: it sits far above the few
/// hundred bytes an AppSpec file takes, and keeps a document nobody could have
/// written as an AppSpec from reaching the parser at all.
const MAX_APP_SPEC_BYTES: usize = 64 * 1024;

/// The longest revision content this emulator parses an AppSpec out of; see
/// [`MAX_APP_SPEC_BYTES`].
///
/// Public so a reader that FETCHES the document rather than being handed it
/// ([`super::s3_revision`]) can apply the same bound to the stored bytes BEFORE it
/// copies them, instead of stating a second bound of its own.
#[must_use]
pub fn max_app_spec_bytes() -> usize {
    MAX_APP_SPEC_BYTES
}

/// How deeply this emulator parses an AppSpec: the nesting a parser would walk,
/// bounded on the TEXT before it sees it — the flow collection openers (`[`, `{`),
/// the block sequence and complex-key indicators (`-`, `?`) standing on their own,
/// and the indentation a block mapping nests by.
///
/// Each level of nesting costs as little as a byte, so the byte bound above does
/// not bound the depth such a document reaches: this does, before either parser
/// sees the content. Also EMULATOR-CHOSEN, an order of magnitude above the handful
/// an AppSpec nests.
const MAX_APP_SPEC_NESTING: usize = 256;

/// The deepest AppSpec document this emulator reads, measured on the parser's own
/// event stream before the loader builds a tree of it.
///
/// EMULATOR-CHOSEN: an AppSpec's own nesting is the document, its `Resources`
/// list, one entry, that resource's body and its `Properties` — five levels — so
/// this sits well above any document that declares a traffic shift, and keeps the
/// loader (and the recursive drop of what it built) off a depth no AppSpec has.
const MAX_APP_SPEC_DEPTH: usize = 32;

/// The traffic shift one Lambda deployment performs, as its revision's AppSpec
/// declares it.
///
/// Read when the deployment is recorded and then held, so the shift a deployment
/// performs is fixed by the revision it was created with — editing nothing and
/// re-reading nothing later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LambdaTrafficShift {
    /// The function whose alias moves: the AppSpec's `Name`.
    pub function_name: String,
    /// The alias that moves: the AppSpec's `Alias`.
    pub alias_name: String,
    /// The version the alias invoked before the deployment: the AppSpec's
    /// `CurrentVersion`, when it states one.
    pub current_version: Option<String>,
    /// The version the alias invokes once the shift is complete: the AppSpec's
    /// `TargetVersion`.
    pub target_version: String,
}

/// The traffic shift `revision` declares for a deployment of `compute_platform`,
/// or `None` when it declares none.
///
/// Only an Lambda deployment shifts traffic between function versions, so the
/// platform is what decides whether the revision's content is read as an Lambda
/// AppSpec at all ([`declared_in_revision`] is that read).
#[must_use]
pub fn declared_shift(
    compute_platform: &str,
    revision: &RevisionLocation,
) -> Option<LambdaTrafficShift> {
    if compute_platform != COMPUTE_PLATFORM_LAMBDA {
        return None;
    }
    declared_in_revision(revision)
}

/// The traffic shift `revision` declares, read out of the content it carries.
///
/// Only the revision forms that CARRY the AppSpec — `appSpecContent`, and the
/// `string` form the model describes as "the location of an Lambda deployment
/// revision stored as a RawString"
/// (`smithy_query(query="shape",shape="RevisionLocation")`) — carry it where this
/// emulator can read it. A revision that names an S3 object or a GitHub commit
/// keeps the AppSpec in that artifact, which this emulator does not fetch, so it
/// declares no shift here.
#[must_use]
pub fn declared_in_revision(revision: &RevisionLocation) -> Option<LambdaTrafficShift> {
    declared_in_app_spec(revision_content(revision)?)
}

/// The AppSpec document `revision` CARRIES, or `None` for a revision that keeps
/// it somewhere this emulator does not read.
///
/// Only the revision forms that carry the document — `appSpecContent`, and the
/// `string` form the model describes as "the location of an Lambda deployment
/// revision stored as a RawString"
/// (`smithy_query(query="shape",shape="RevisionLocation")`) — carry it where this
/// emulator can read it. A revision that names an S3 object or a GitHub commit
/// keeps the AppSpec in that artifact, which this emulator does not fetch.
#[must_use]
pub fn revision_content(revision: &RevisionLocation) -> Option<&str> {
    revision
        .app_spec_content
        .as_ref()
        .and_then(|content| content.content.as_deref())
        .or_else(|| {
            revision
                .string
                .as_ref()
                .and_then(|content| content.content.as_deref())
        })
}

/// The traffic shift an AppSpec document declares, or `None` for content that
/// declares none — including content that is no YAML (or JSON, which YAML 1.2 is a
/// superset of) document at all, and content outside the bounds this emulator
/// parses one within.
///
/// Public so a test can hold the reading of a document to this rule without
/// standing a deployment up for every row.
#[must_use]
pub fn declared_in_app_spec(content: &str) -> Option<LambdaTrafficShift> {
    if !within_bounds(content) || !parses_within_depth(content) {
        return None;
    }
    ::yaml_rust2::YamlLoader::load_from_str(content)
        .ok()?
        .iter()
        .find_map(declared_in_document)
}

/// One lifecycle event hook a revision's AppSpec declares: the lifecycle event,
/// and the Lambda validation function that runs for it.
///
/// "A deployment lifecycle hook is specified in the hooks section of the AppSpec
/// file"
/// (`smithy_query(query="doc",shape="PutLifecycleEventHookExecutionStatusInput",member="lifecycleEventHookExecutionId")`),
/// and what it names for a Lambda or Amazon ECS deployment is the validation
/// function whose result that event's outcome IS — the function CodeDeploy invokes
/// with the event, and that reports back through
/// `PutLifecycleEventHookExecutionStatus`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredHook {
    /// Which lifecycle event the hook runs for, as the document spells it.
    pub lifecycle_event_name: String,
    /// The validation function the document names for it.
    pub function_name: String,
}

/// The lifecycle event hooks `revision` declares, in document order.
///
/// Read when the deployment is recorded and then held, exactly as the traffic
/// shift is, so the hooks a deployment runs are fixed by the revision it was
/// created with.
#[must_use]
pub fn declared_hooks_in_revision(revision: &RevisionLocation) -> Vec<DeclaredHook> {
    revision_content(revision).map_or_else(Vec::new, declared_hooks_in_app_spec)
}

/// The lifecycle event hooks one AppSpec document declares, in document order;
/// empty for content that declares none, and for content outside the bounds this
/// emulator parses a document within ([`declared_in_app_spec`] states them).
///
/// Public so a test can hold the reading of a document to this rule without
/// standing a deployment up for every row.
#[must_use]
pub fn declared_hooks_in_app_spec(content: &str) -> Vec<DeclaredHook> {
    if !within_bounds(content) || !parses_within_depth(content) {
        return Vec::new();
    }
    ::yaml_rust2::YamlLoader::load_from_str(content)
        .unwrap_or_default()
        .iter()
        .flat_map(hooks_in_document)
        .collect()
}

/// The hooks one document's `Hooks` section declares.
///
/// The section is written as a sequence of single-entry mappings (the form AWS
/// documents) or as one mapping of event name to function, so both are read.
fn hooks_in_document(document: &Yaml) -> Vec<DeclaredHook> {
    let Some(hooks) = mapping_get_ignoring_case(document, HOOKS_KEY) else {
        return Vec::new();
    };
    match hooks {
        Yaml::Array(entries) => entries.iter().flat_map(hooks_in_entry).collect(),
        mapping => hooks_in_entry(mapping),
    }
}

/// The hooks one entry of that section declares: each key is the lifecycle event
/// and each value the validation function it runs.
///
/// A value that is no scalar names no validation function — an EC2/on-premises
/// AppSpec's hooks are a sequence of script locations the CodeDeploy agent runs,
/// not a function this service invokes — so such an entry declares nothing here.
fn hooks_in_entry(entry: &Yaml) -> Vec<DeclaredHook> {
    let Yaml::Hash(declared) = entry else {
        return Vec::new();
    };
    declared
        .iter()
        .filter_map(|(event, function)| {
            let lifecycle_event_name = event.as_str()?.trim();
            let function_name = scalar(function)?;
            let function_name = function_name.trim();
            (!lifecycle_event_name.is_empty() && !function_name.is_empty()).then(|| DeclaredHook {
                lifecycle_event_name: lifecycle_event_name.to_string(),
                function_name: function_name.to_string(),
            })
        })
        .collect()
}

/// Whether the content is small enough and flat enough to hand to a parser at
/// all: the two bounds on work a caller chooses the size of
/// ([`MAX_APP_SPEC_BYTES`], [`MAX_APP_SPEC_NESTING`]), checked on the bytes before
/// any parser sees them.
///
/// The nesting is bounded by the shared scan every service that reads a
/// caller-supplied YAML document this way uses (`service_common::yaml_nesting`),
/// rather than by a scan of this crate's own.
fn within_bounds(content: &str) -> bool {
    content.len() <= MAX_APP_SPEC_BYTES
        && ::service_common::yaml_nesting::nesting_depth_bound(content, MAX_APP_SPEC_NESTING)
            <= MAX_APP_SPEC_NESTING
}

/// Whether the document parses, uses no YAML alias, and nests no deeper than
/// [`MAX_APP_SPEC_DEPTH`] — all three read off the parser's own event stream,
/// which copies nothing and builds no tree.
///
/// The depth is measured HERE rather than on the loaded document because the
/// loader resolves an alias by COPYING the anchored node and builds (and then
/// recursively drops) the whole tree: by the time there is a tree to measure, the
/// expansion and the recursion have already happened.
fn parses_within_depth(content: &str) -> bool {
    use ::yaml_rust2::parser::Event;

    #[derive(Default)]
    struct Guard {
        /// Whether an alias was reported, whose expansion copies a node.
        saw_alias: bool,
        /// How many collections are open at this event.
        depth: usize,
        /// The most that were open at once.
        deepest: usize,
    }
    impl ::yaml_rust2::parser::EventReceiver for Guard {
        fn on_event(&mut self, event: Event) {
            match event {
                Event::Alias(_) => self.saw_alias = true,
                Event::SequenceStart(..) | Event::MappingStart(..) => {
                    self.depth = self.depth.saturating_add(1);
                    self.deepest = self.deepest.max(self.depth);
                }
                Event::SequenceEnd | Event::MappingEnd => {
                    self.depth = self.depth.saturating_sub(1);
                }
                _ => {}
            }
        }
    }

    let mut guard = Guard::default();
    let parsed = ::yaml_rust2::parser::Parser::new_from_str(content).load(&mut guard, true);
    parsed.is_ok() && !guard.saw_alias && guard.deepest <= MAX_APP_SPEC_DEPTH
}

/// The traffic shift one AppSpec document declares: the first `Resources` entry
/// that describes a Lambda function.
fn declared_in_document(document: &Yaml) -> Option<LambdaTrafficShift> {
    let resources = mapping_get(document, RESOURCES_KEY)?;
    match resources {
        Yaml::Array(declarations) => declarations.iter().find_map(declared_in_declaration),
        mapping => declared_in_declaration(mapping),
    }
}

/// The traffic shift one `Resources` entry declares.
///
/// An AppSpec names the resource it deploys, so an entry is a mapping from that
/// logical name to the resource's own body; an entry that IS the body carries the
/// `Properties` mapping itself, and is read the same way.
fn declared_in_declaration(declaration: &Yaml) -> Option<LambdaTrafficShift> {
    let Yaml::Hash(named) = declaration else {
        return None;
    };
    named
        .iter()
        .map(|(_, body)| body)
        .chain(::std::iter::once(declaration))
        .find_map(declared_in_body)
}

/// The traffic shift one resource body declares: its `Properties`, when the body
/// is a Lambda function at all.
///
/// The resource's `Type` is REQUIRED to be the Lambda function one: a body that
/// states another type describes no version shift, and a body that states no type
/// is no resource declaration this emulator reads a shift out of.
fn declared_in_body(body: &Yaml) -> Option<LambdaTrafficShift> {
    if mapping_get(body, TYPE_KEY)
        .and_then(scalar)
        .is_none_or(|declared| declared.trim() != LAMBDA_FUNCTION_RESOURCE_TYPE)
    {
        return None;
    }
    let properties = mapping_get(body, PROPERTIES_KEY)?;
    let property = |name: &str| {
        mapping_get(properties, name)
            .and_then(scalar)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    Some(LambdaTrafficShift {
        function_name: property(NAME_PROPERTY)?,
        alias_name: property(ALIAS_PROPERTY)?,
        current_version: property(CURRENT_VERSION_PROPERTY),
        target_version: property(TARGET_VERSION_PROPERTY)?,
    })
}

/// One key of a YAML mapping; `None` for a node that is no mapping, and for a
/// mapping that does not carry the key.
fn mapping_get<'a>(node: &'a Yaml, key: &str) -> Option<&'a Yaml> {
    let Yaml::Hash(mapping) = node else {
        return None;
    };
    mapping
        .iter()
        .find(|(stored, _)| stored.as_str() == Some(key))
        .map(|(_, value)| value)
}

/// [`mapping_get`] without regard to the key's case; see [`HOOKS_KEY`].
fn mapping_get_ignoring_case<'a>(node: &'a Yaml, key: &str) -> Option<&'a Yaml> {
    let Yaml::Hash(mapping) = node else {
        return None;
    };
    mapping
        .iter()
        .find(|(stored, _)| stored.as_str().is_some_and(|stored| stored.eq_ignore_ascii_case(key)))
        .map(|(_, value)| value)
}

/// One AppSpec scalar as the text the property carries.
///
/// A version number written unquoted is a YAML integer, and the member it fills —
/// Lambda's own `Version` (pattern `^(\$LATEST|[0-9]+)$`) — is text, so the
/// integer spelling is read as the digits it is.
fn scalar(node: &Yaml) -> Option<String> {
    match node {
        Yaml::String(text) | Yaml::Real(text) => Some(text.clone()),
        Yaml::Integer(number) => Some(number.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    //! What [`declared_in_app_spec`] reads out of a revision's content, over the
    //! forms an AppSpec is written in — cheap here, where a row costs a parse
    //! rather than a function, three versions, an alias and a deployment.

    use super::*;

    /// A shift with no `CurrentVersion`, for the rows that state none.
    fn shift(function_name: &str, alias_name: &str, target_version: &str) -> LambdaTrafficShift {
        LambdaTrafficShift {
            function_name: function_name.to_string(),
            alias_name: alias_name.to_string(),
            current_version: None,
            target_version: target_version.to_string(),
        }
    }

    /// RULE: the AppSpec's Lambda function declaration is what names the function,
    /// the alias and the versions of the shift — in either of the two formats the
    /// model says the content is written in, with the version numbers written as
    /// text or as numbers.
    #[test]
    fn the_app_spec_declares_the_shift() {
        let yaml = "version: 0.0\n\
                    Resources:\n\
                    \x20 - handler:\n\
                    \x20     Type: AWS::Lambda::Function\n\
                    \x20     Properties:\n\
                    \x20       Name: handler\n\
                    \x20       Alias: live\n\
                    \x20       CurrentVersion: \"1\"\n\
                    \x20       TargetVersion: \"2\"\n";
        let unquoted = "version: 0.0\n\
                        Resources:\n\
                        \x20 - handler:\n\
                        \x20     Type: AWS::Lambda::Function\n\
                        \x20     Properties:\n\
                        \x20       Name: handler\n\
                        \x20       Alias: live\n\
                        \x20       CurrentVersion: 1\n\
                        \x20       TargetVersion: 2\n";
        let json = r#"{"version": 0.0, "Resources": [{"handler": {
            "Type": "AWS::Lambda::Function",
            "Properties": {"Name": "handler", "Alias": "live",
                           "CurrentVersion": "1", "TargetVersion": "2"}}}]}"#;
        let mapping = "version: 0.0\n\
                       Resources:\n\
                       \x20 handler:\n\
                       \x20   Type: AWS::Lambda::Function\n\
                       \x20   Properties:\n\
                       \x20     Name: orders\n\
                       \x20     Alias: blue\n\
                       \x20     TargetVersion: 7\n";
        let ecs = "version: 0.0\n\
                   Resources:\n\
                   \x20 - target-service:\n\
                   \x20     Type: AWS::ECS::Service\n\
                   \x20     Properties:\n\
                   \x20       TaskDefinition: arn:aws:ecs:us-east-1:1234:task-definition/web:1\n";
        let aliased = "version: &anchor 0.0\n\
                       Resources: *anchor\n";

        let declared = shift("handler", "live", "2");
        let with_current = LambdaTrafficShift {
            current_version: Some("1".to_string()),
            ..declared
        };
        let cases: &[(&str, &str, Option<LambdaTrafficShift>)] = &[
            ("a YAML AppSpec", yaml, Some(with_current.clone())),
            (
                "a YAML AppSpec whose versions are unquoted numbers",
                unquoted,
                Some(with_current.clone()),
            ),
            ("a JSON AppSpec", json, Some(with_current)),
            (
                "a YAML AppSpec whose Resources is a mapping",
                mapping,
                Some(shift("orders", "blue", "7")),
            ),
            ("an AppSpec that declares no resource", "version: 0.0", None),
            ("an AppSpec of another compute platform", ecs, None),
            ("content that is no document at all", "\tnot: yaml\n", None),
            ("an AppSpec that uses a YAML alias", aliased, None),
            ("empty content", "", None),
        ];
        for (case, content, expected) in cases {
            let read = declared_in_app_spec(content);
            assert_eq!(
                read.as_ref(),
                expected.as_ref(),
                "case {case:?}: expected the declared shift {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: content outside the bounds this emulator parses an AppSpec within is
    /// not parsed at all, so it declares no shift — each bound held at its own
    /// value and one step past it.
    #[test]
    fn content_outside_the_parsing_bounds_declares_no_shift() {
        let document = "version: 0.0\n\
                        Resources:\n\
                        \x20 - handler:\n\
                        \x20     Type: AWS::Lambda::Function\n\
                        \x20     Properties:\n\
                        \x20       Name: handler\n\
                        \x20       Alias: live\n\
                        \x20       TargetVersion: 2\n";
        // The same AppSpec, padded with a comment line to exactly the byte bound
        // and to one byte past it: the comment is `# `, the padding, and the
        // newline that ends it, so three bytes beside the padding itself.
        let padded = |len: usize| {
            let mut content = String::from("# ");
            content.push_str(&"=".repeat(len - document.len() - 3));
            content.push('\n');
            content.push_str(document);
            content
        };
        let at_bytes = padded(MAX_APP_SPEC_BYTES);
        let past_bytes = padded(MAX_APP_SPEC_BYTES + 1);
        // A block sequence of `n` entries, which is `n` nesting indicators plus
        // the two columns each entry is indented at: the spelling the byte bound
        // alone would not hold, since each entry costs a handful of bytes.
        let entries = |count: usize| {
            let mut content = String::from("Resources:\n");
            for _ in 0..count {
                content.push_str("  - a\n");
            }
            content
        };
        let at_nesting = entries(MAX_APP_SPEC_NESTING - 2);
        let past_nesting = entries(MAX_APP_SPEC_NESTING - 1);
        // A block mapping nested by INDENTATION alone, which carries no nesting
        // indicator at all and is bounded by the columns it is written at.
        let indented = |depth: usize| {
            (0..depth)
                .map(|level| format!("{}k{level}:\n", " ".repeat(level)))
                .collect::<String>()
        };
        let at_indentation = indented(MAX_APP_SPEC_NESTING + 1);
        let past_indentation = indented(MAX_APP_SPEC_NESTING + 2);
        // A document nested to a chosen depth: the mapping it opens with, plus one
        // level per flow collection inside it.
        let nested = |depth: usize| {
            format!(
                "Resources: {}{}",
                "[".repeat(depth - 1),
                "]".repeat(depth - 1)
            )
        };
        let at_depth = nested(MAX_APP_SPEC_DEPTH);
        let past_depth = nested(MAX_APP_SPEC_DEPTH + 1);

        let declared = shift("handler", "live", "2");
        let cases: &[(&str, &str, bool, bool, Option<&LambdaTrafficShift>)] = &[
            (
                "an AppSpec padded to exactly the byte bound",
                &at_bytes,
                true,
                true,
                Some(&declared),
            ),
            (
                "an AppSpec padded one byte past the byte bound",
                &past_bytes,
                false,
                true,
                None,
            ),
            (
                "a document at the nesting bound",
                &at_nesting,
                true,
                true,
                None,
            ),
            (
                "a document one entry past the nesting bound",
                &past_nesting,
                false,
                true,
                None,
            ),
            (
                "a block mapping indented to exactly the nesting bound",
                &at_indentation,
                true,
                // Inside the TEXT bound and still far past the DEPTH bound the
                // parser's own event stream measures: the scan is generous where
                // the parser is exact, which is the safe direction for a bound
                // taken before the parse.
                false,
                None,
            ),
            (
                "a block mapping indented one column past the nesting bound",
                &past_indentation,
                false,
                false,
                None,
            ),
            (
                "a document nested to exactly the depth bound",
                &at_depth,
                true,
                true,
                None,
            ),
            (
                "a document nested one level past the depth bound",
                &past_depth,
                true,
                false,
                None,
            ),
        ];

        for (case, content, bounded, within_depth, expected) in cases {
            let within = within_bounds(content);
            assert_eq!(
                within, *bounded,
                "case {case:?}: expected the content to be within bounds: {bounded}, got {within}"
            );
            let depth = parses_within_depth(content);
            assert_eq!(
                depth, *within_depth,
                "case {case:?}: expected the content to be within the depth bound: \
                 {within_depth}, got {depth}"
            );
            let read = declared_in_app_spec(content);
            assert_eq!(
                read.as_ref(),
                *expected,
                "case {case:?}: expected the declared shift {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: only an Lambda deployment shifts traffic between function versions,
    /// and only the revision forms that carry the AppSpec carry the shift.
    #[test]
    fn only_a_lambda_revision_that_carries_the_app_spec_declares_a_shift() {
        let content = "version: 0.0\n\
                       Resources:\n\
                       \x20 - handler:\n\
                       \x20     Type: AWS::Lambda::Function\n\
                       \x20     Properties:\n\
                       \x20       Name: handler\n\
                       \x20       Alias: live\n\
                       \x20       TargetVersion: 2\n";
        let app_spec_content = RevisionLocation {
            revision_type: Some("AppSpecContent".to_string()),
            app_spec_content: Some(crate::ops::types::AppSpecContent {
                content: Some(content.to_string()),
                sha256: None,
            }),
            ..RevisionLocation::default()
        };
        let raw_string = RevisionLocation {
            revision_type: Some("String".to_string()),
            string: Some(crate::ops::types::RawString {
                content: Some(content.to_string()),
                sha256: None,
            }),
            ..RevisionLocation::default()
        };
        let s3 = RevisionLocation {
            revision_type: Some("S3".to_string()),
            s3_location: Some(crate::ops::types::S3Location {
                bucket: Some("artifacts".to_string()),
                key: Some("app.zip".to_string()),
                ..crate::ops::types::S3Location::default()
            }),
            ..RevisionLocation::default()
        };

        let declared = shift("handler", "live", "2");
        let cases: &[(&str, &str, &RevisionLocation, Option<LambdaTrafficShift>)] = &[
            (
                "an AppSpecContent revision of a Lambda deployment",
                COMPUTE_PLATFORM_LAMBDA,
                &app_spec_content,
                Some(declared.clone()),
            ),
            (
                "a String revision of a Lambda deployment",
                COMPUTE_PLATFORM_LAMBDA,
                &raw_string,
                Some(declared),
            ),
            (
                "an S3 revision of a Lambda deployment",
                COMPUTE_PLATFORM_LAMBDA,
                &s3,
                None,
            ),
            (
                "the same AppSpec on an EC2/on-premises deployment",
                "Server",
                &app_spec_content,
                None,
            ),
            (
                "the same AppSpec on an Amazon ECS deployment",
                "ECS",
                &app_spec_content,
                None,
            ),
        ];

        for (case, platform, revision, expected) in cases {
            let read = declared_shift(platform, revision);
            assert_eq!(
                read.as_ref(),
                expected.as_ref(),
                "case {case:?}: expected the declared shift {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: the AppSpec's `Hooks` section is what names the validation function
    /// of each lifecycle event, in either format the content is written in and in
    /// either spelling of that section — and a hook that names no function (an
    /// EC2/on-premises AppSpec's script locations, say) declares none.
    #[test]
    fn the_app_spec_declares_the_lifecycle_event_hooks() {
        let both = "version: 0.0\n\
                    Hooks:\n\
                    \x20 - BeforeAllowTraffic: \"check-before\"\n\
                    \x20 - AfterAllowTraffic: \"check-after\"\n";
        let mapping = "version: 0.0\n\
                       Hooks:\n\
                       \x20 BeforeAllowTraffic: check-before\n";
        let json = r#"{"version": 0.0, "Hooks": [{"AfterAllowTraffic": "check-after"}]}"#;
        let ecs = "version: 0.0\n\
                   Hooks:\n\
                   \x20 - AfterAllowTestTraffic: \"check-test-traffic\"\n";
        let agent = "version: 0.0\n\
                     hooks:\n\
                     \x20 AfterInstall:\n\
                     \x20   - location: scripts/install.sh\n";
        let scripts = "version: 0.0\n\
                       Hooks:\n\
                       \x20 AfterInstall:\n\
                       \x20   - location: scripts/install.sh\n";
        let lower_case = "version: 0.0\n\
                          hooks:\n\
                          \x20 - BeforeAllowTraffic: check-before\n";

        let hook = |event: &str, function: &str| DeclaredHook {
            lifecycle_event_name: event.to_string(),
            function_name: function.to_string(),
        };
        let cases: &[(&str, &str, Vec<DeclaredHook>)] = &[
            (
                "both Lambda hooks",
                both,
                vec![
                    hook("BeforeAllowTraffic", "check-before"),
                    hook("AfterAllowTraffic", "check-after"),
                ],
            ),
            (
                "a Hooks section written as one mapping",
                mapping,
                vec![hook("BeforeAllowTraffic", "check-before")],
            ),
            (
                "a JSON AppSpec",
                json,
                vec![hook("AfterAllowTraffic", "check-after")],
            ),
            (
                "an Amazon ECS lifecycle event",
                ecs,
                vec![hook("AfterAllowTestTraffic", "check-test-traffic")],
            ),
            ("an AppSpec that declares no hook", "version: 0.0", vec![]),
            (
                // The AppSpec reference spells the section `hooks` for
                // EC2/on-premises, so the section is read whichever case it
                // carries; its ENTRIES here name script locations rather than a
                // validation function, so they declare none.
                "a lower-case hooks section naming script locations",
                agent,
                vec![],
            ),
            (
                "a lower-case hooks section naming a function",
                lower_case,
                vec![hook("BeforeAllowTraffic", "check-before")],
            ),
            ("hooks that name script locations", scripts, vec![]),
            ("content that is no document at all", "\tnot: yaml\n", vec![]),
        ];

        for (case, content, expected) in cases {
            let read = declared_hooks_in_app_spec(content);
            assert_eq!(
                &read, expected,
                "case {case:?}: expected the declared hooks {expected:?}, got {read:?}"
            );
        }
    }
}
