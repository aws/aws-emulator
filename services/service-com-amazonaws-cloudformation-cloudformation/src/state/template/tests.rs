// SPDX-License-Identifier: CC0-1.0

use super::*;

/// The attributes [`return_values`] computes for a resource of `resource_type`.
///
/// `return_values` reads the whole declaration, because a few published return values
/// are functions of the resource's own properties: a DB type's `Endpoint.Port` needs a
/// declared engine, and the two NESTED Elastic Load Balancing types (a listener and a
/// rule, whose ARNs are built from their parent's) the parent ARN. So the declaration
/// here carries what a template declaring one of those types would, which is what
/// makes the attribute set it yields the type's WHOLE published set.
fn computed_attributes(resource_type: &str) -> BTreeMap<String, String> {
    let stack = StackScope {
        region: "us-east-1",
        stack_name: "a-stack",
        stack_id: "a-stack-id",
        notification_arns: &[],
    };
    let parameters = BTreeMap::new();
    let parameter_types = BTreeMap::new();
    let resources = BTreeMap::new();
    let mappings = Value::Null;
    let conditions = Value::Null;
    let imports = BTreeMap::new();
    let stack_outputs = crate::state::stack_outputs::Values::new();
    let scope = Scope {
        stack,
        parameters: &parameters,
        parameter_types: &parameter_types,
        resources: &resources,
        mappings: &mappings,
        conditions: &conditions,
        imports: &imports,
        stack_outputs: &stack_outputs,
    };
    let mut budget = Budget::new();
    let declared = DeclaredResource {
        logical_id: "ARes".to_string(),
        resource_type: resource_type.to_string(),
        properties: ::serde_json::json!({
            "Engine": "aurora-mysql",
            // The two required properties of an `AWS::SSM::Parameter`, which are also
            // the two values that type publishes.
            "Type": "String",
            "Value": "a-value",
            "LoadBalancerArn": format!(
                "arn:{PARTITION}:elasticloadbalancing:us-east-1:000000000000:\
                     loadbalancer/app/a-name/0123456789abcdef"
            ),
            "ListenerArn": format!(
                "arn:{PARTITION}:elasticloadbalancing:us-east-1:000000000000:\
                     listener/app/a-name/0123456789abcdef/fedcba9876543210"
            ),
            // The vector bucket an `AWS::S3Vectors::Index` declares its index in,
            // whose ARN that index's own ARN nests inside.
            "VectorBucketName": "a-vector-bucket",
            // The CodeArtifact domain an `AWS::CodeArtifact::Repository` declares its
            // repository in, whose ARN that repository's own ARN nests inside.
            "DomainName": "a-domain",
            // The function an `AWS::Lambda::Version` and an `AWS::Lambda::Alias` declare
            // against, whose ARN each of their own ARNs is a qualified form of.
            "FunctionName": "a-function",
            // The document an `AWS::Route53::TrafficPolicy` declares, whose own `RecordType`
            // is the `Type` that type publishes.
            "Document": { "RecordType": "A" },
            // The app environment an `AWS::AmplifyUIBuilder::{Component,Form,Theme}` declares
            // its entity in, which are two of the three values each of those types publishes.
            "AppId": "d1234abcdefghi",
            "EnvironmentName": "staging",
        }),
        condition: None,
        declaration: Value::Null,
    };
    let (_, attributes) = return_values(&declared, "a-name", &scope, &mut budget);
    attributes
}

/// [`IMPLEMENTED_RESOURCE_TYPES`] is the catalogue of the arms of [`return_values`],
/// and the registry reports a published extension for exactly its entries — so an
/// entry that `return_values` does not actually know would make the registry report an
/// extension for a type this emulator provisions no better than any other. Every
/// catalogued type must therefore yield the attributes its "Return values" section
/// publishes, which a type outside the `match` has none of.
#[test]
fn every_implemented_resource_type_has_known_return_values() {
    for resource_type in IMPLEMENTED_RESOURCE_TYPES {
        let attributes = computed_attributes(resource_type);
        assert!(
            !attributes.is_empty(),
            "case {resource_type:?}: expected the type's documented Fn::GetAtt \
                 attributes, got none — the catalogue and the return_values match have \
                 drifted apart"
        );
    }
    // And a type no arm carries is NOT in the catalogue: it has no attributes at all.
    let unknown = computed_attributes("AWS::NotAService::NotAType");
    assert!(
        unknown.is_empty(),
        "case \"a type no arm carries\": expected no attributes, got {unknown:?}"
    );
}

/// The physical name the naming rules give one declaration, for the table below.
fn named(resource_type: &str, properties: Value) -> String {
    let stack = StackScope {
        region: "us-east-1",
        stack_name: "a-stack",
        stack_id: "a-stack-id",
        notification_arns: &[],
    };
    let parameters = BTreeMap::new();
    let parameter_types = BTreeMap::new();
    let resources = BTreeMap::new();
    let mappings = Value::Null;
    let conditions = Value::Null;
    let imports = BTreeMap::new();
    let stack_outputs = crate::state::stack_outputs::Values::new();
    let scope = Scope {
        stack,
        parameters: &parameters,
        parameter_types: &parameter_types,
        resources: &resources,
        mappings: &mappings,
        conditions: &conditions,
        imports: &imports,
        stack_outputs: &stack_outputs,
    };
    let mut budget = Budget::new();
    let declared = DeclaredResource {
        logical_id: "ARes".to_string(),
        resource_type: resource_type.to_string(),
        properties,
        condition: None,
        declaration: Value::Null,
    };
    physical_name(&declared, &scope, &mut budget)
}

/// A resource carries the name its declaration states, wherever the type states it:
/// through one of the conventions ([`name_property_candidates`]), through the type's own
/// unconventional name property ([`declared_name_property`]), or inside the INPUT
/// STRUCTURE its owning API takes that name in ([`declared_name_path`]) — and only a
/// declaration that names the resource NOWHERE gets a generated
/// `<stack>-<logical id>-<unique id>`.
///
/// The rows span the forms a declaration can state it in: each of the three places, a
/// name written as an intrinsic rather than a literal, and a declaration that carries
/// the input structure but no name inside it.
#[test]
fn a_name_is_read_from_wherever_the_type_states_it() {
    // (case, the type, the declared properties, the name it must carry — `None` for a
    // declaration whose name CloudFormation generates).
    let rows: Vec<(&str, &str, Value, Option<&str>)> = vec![
        (
            "the convention: the type's last segment plus Name",
            SQS_QUEUE,
            ::serde_json::json!({ "QueueName": "a-queue" }),
            Some("a-queue"),
        ),
        (
            "the convention: a plain Name",
            GLUE_CRAWLER,
            ::serde_json::json!({ "Name": "a-crawler" }),
            Some("a-crawler"),
        ),
        (
            "the type's own unconventional name property",
            ECR_PUBLIC_REPOSITORY,
            ::serde_json::json!({ "RepositoryName": "a-repository" }),
            Some("a-repository"),
        ),
        (
            "a name inside the input structure the owning API takes it in",
            GLUE_DATABASE,
            ::serde_json::json!({
                "DatabaseInput": { "Name": "a_database", "Description": "a description" }
            }),
            Some("a_database"),
        ),
        (
            "the same for a table, beside the database property that is not its name",
            GLUE_TABLE,
            ::serde_json::json!({
                "DatabaseName": "a_database",
                "TableInput": { "Name": "a_table" }
            }),
            Some("a_table"),
        ),
        (
            "a nested name written as an intrinsic, which is evaluated like any other",
            GLUE_TABLE,
            ::serde_json::json!({
                "TableInput": { "Name": { "Fn::Sub": "a_${AWS::Region}_table" } }
            }),
            Some("a_us-east-1_table"),
        ),
        (
            "an input structure carrying no name, which names the resource nowhere",
            GLUE_DATABASE,
            ::serde_json::json!({ "DatabaseInput": { "Description": "a description" } }),
            None,
        ),
        (
            "a declaration with no properties at all",
            GLUE_CRAWLER,
            ::serde_json::json!({}),
            None,
        ),
    ];

    for (case, resource_type, properties, expected) in rows {
        let actual = named(resource_type, properties);
        match expected {
            Some(declared) => assert_eq!(
                actual, declared,
                "case {case:?}: expected the declared name {declared:?}, got {actual:?}"
            ),
            None => assert!(
                actual.starts_with("a-stack-ARes-") && actual.len() > "a-stack-ARes-".len(),
                "case {case:?}: expected a generated <stack>-<logical id>-<unique id>, got \
                     {actual:?}"
            ),
        }
    }
}

/// Every site [`substitute_identifier`] rewrites is one [`identifier_reads`] counts, and the
/// count is the same whichever characters the loop identifier carries.
///
/// The expansion budget is charged per item from that count — each read grows the instance
/// by the item's own length — so a count that misses a site lets one submitted template
/// materialise more bytes than the ceiling allows. The identifier is the template author's
/// string, and the serialized spelling of one carrying `"`, `\` or a control byte differs
/// from its spelling in the data the substitution walks, so the two halves of each pair
/// below must agree.
#[test]
fn a_loop_identifier_is_counted_wherever_substitution_rewrites_it() {
    // (case, the identifier, one output map value, the reads it holds)
    let rows: &[(&str, &str, Value, usize)] = &[
        (
            "a nested key reading the identifier",
            "N",
            ::serde_json::json!({ "Name${N}": { "Type": "A::B::C" } }),
            1,
        ),
        (
            "an Fn::Sub template string reading it twice",
            "N",
            ::serde_json::json!({ "Fn::Sub": "${N}-${N}" }),
            2,
        ),
        (
            "a two-argument Fn::Sub whose variables map reads it as a Ref",
            "N",
            ::serde_json::json!({ "Fn::Sub": ["${Zone}-${N}", { "Zone": { "Ref": "N" } }] }),
            2,
        ),
        (
            "a single-member Ref node, replaced by the item whole",
            "N",
            ::serde_json::json!({ "Properties": { "Value": { "Ref": "N" } } }),
            1,
        ),
        (
            "a plain string value, which substitution does not rewrite",
            "N",
            ::serde_json::json!({ "Properties": { "Value": "${N}" } }),
            0,
        ),
        (
            "a list of values, each reading it",
            "N",
            ::serde_json::json!({ "Items": [{ "Ref": "N" }, { "Fn::Sub": "${N}" }] }),
            2,
        ),
        // The same shapes under an identifier whose JSON spelling is escaped: the counts
        // are properties of the DATA, so they do not change.
        (
            "a nested key read by an identifier carrying a quote",
            "N\"q",
            ::serde_json::json!({ "Name${N\"q}": { "Type": "A::B::C" } }),
            1,
        ),
        (
            "an Fn::Sub string read twice by an identifier carrying a backslash",
            "N\\b",
            ::serde_json::json!({ "Fn::Sub": "${N\\b}-${N\\b}" }),
            2,
        ),
        (
            "a Ref node naming an identifier carrying a quote",
            "N\"q",
            ::serde_json::json!({ "Properties": { "Value": { "Ref": "N\"q" } } }),
            1,
        ),
    ];

    for (case, identifier, value, expected) in rows {
        let needle = format!("${{{identifier}}}");
        let actual = identifier_reads(value, &needle, Some(identifier), 0);
        assert_eq!(
            actual, *expected,
            "case {case:?}: expected {expected} read(s) of {needle:?}, got {actual}"
        );
        // The alternate `&{…}` form this service does not apply is counted the same way —
        // it is what the expander refuses a template on — and a `Ref` node is not one of
        // its sites, which is why that half is counted without the identifier.
        let alternate = format!("&{{{identifier}}}");
        let alternate_reads = identifier_reads(value, &alternate, None, 0);
        assert_eq!(
            alternate_reads, 0,
            "case {case:?}: expected no read of {alternate:?} in a value that spells only \
             {needle:?}, got {alternate_reads}"
        );
    }

    // And the alternate form IS found where a key spells it, whatever the identifier
    // carries: the refusal that reports it reads the same count.
    for (case, identifier) in [
        ("a plain identifier", "N"),
        ("an identifier carrying a quote", "N\"q"),
    ] {
        let value = ::serde_json::json!({ format!("Name&{{{identifier}}}"): { "Type": "A::B::C" } });
        let alternate = format!("&{{{identifier}}}");
        let actual = identifier_reads(&value, &alternate, None, 0);
        assert_eq!(
            actual, 1,
            "case {case:?}: expected one read of {alternate:?}, got {actual}"
        );
    }
}

/// [`published_attribute_names`] names exactly the attributes [`return_values`]
/// computes, for every type either half carries: the registry's resource schema is
/// built from the names, so a name missing from one half or invented in the other
/// would make the schema a published type's registry record carries disagree with the
/// attributes a resource of that type actually answers.
#[test]
fn published_attribute_names_are_the_catalogued_return_values() {
    // Every type the schema side is asked about, plus one no arm carries at all.
    let types = IMPLEMENTED_RESOURCE_TYPES
        .iter()
        .copied()
        .chain(["AWS::NotAService::NotAType"]);
    for resource_type in types {
        let computed: Vec<String> = computed_attributes(resource_type).into_keys().collect();
        let named: Vec<String> = published_attribute_names(resource_type)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            named, computed,
            "case {resource_type:?}: expected the names of the attributes \
                 return_values computes ({computed:?}), got {named:?}"
        );
    }
}
