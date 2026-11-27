// SPDX-License-Identifier: CC0-1.0

//! What the `AWS::Serverless-2016-10-31` transform REFUSES: the faults a submitted SAM
//! document carries, and the diagnostic the transform's failure reports them in.
//!
//! ## The rule this module implements
//!
//! A macro either processes a template or FAILS on it, and a transform's failure is not a bad
//! request: the operation that submitted the document is accepted and the work it opened ends
//! in the failure the macro reports — a change set in `FAILED`, a stack in its own failed
//! status — carrying the macro's own diagnostic as the reason, with nothing planned and
//! nothing deployed ([`super::super::template_validation::deployment_failure`] and
//! `crate::ops::create_change_set` are where that reason is answered).
//!
//! The Serverless macro's diagnostic is an AGGREGATE: it validates every `AWS::Serverless::*`
//! declaration of the document, counts the faults it found and names each one by the
//! declaration it is about — `Invalid Serverless Application Specification document. Number of
//! errors found: <n>.` followed by one `Resource with id [<logical id>] is invalid. <what is
//! wrong>` per faulty declaration, all of it wrapped in CloudFormation's own `Transform <name>
//! failed with: ` ([`super::super::private_macro`] words that wrapper for a macro a client
//! registered, and this is the same sentence for the macro this service applies itself).
//!
//! One fault is CloudFormation's rather than the macro's: a declaration whose type is in the
//! Serverless namespace but is no type the macro defines is a type CloudFormation does not
//! recognise at all, reported in the words it reports every unrecognised type in
//! ([`super::super::resource_types::unrecognized_message`]) — the macro is never asked about a
//! document whose resource types do not exist.
//!
//! ## What is judged here, and what is not
//!
//! Only the rules whose violation the macro cannot expand PAST are judged: a declaration
//! stating none of a property whose value the expansion could not supply
//! ([`REQUIRED_PROPERTIES`]), a `Policies` entry
//! that is none of the three things the property accepts, a `Zip` function with no runtime or
//! no handler to run, an `Events` entry naming no event source type (on the two types that
//! take one), a `SimpleTable` whose stated primary key says nothing about its type, an
//! API whose `Auth` states an authorizer that could never authorize a request or a default
//! authorizer the api does not declare, and a CONNECTOR asking for a permission the profile of
//! the pair it runs between does not publish ([`super::connector`]). Everything else a
//! declaration carries is expanded as written and judged where it lands — by the service that
//! owns the resource the expansion produced, which is where the real macro leaves it too: the
//! code a function runs, the definition a state machine runs and the members one `Events` entry
//! states are all judged by the resource each becomes, so a declaration missing one of them is
//! refused there rather than here.
//!
//! At most ONE fault per declaration, which is how the validator reports them: the first thing
//! it finds wrong with a resource is what that resource is invalid for, so the count is the
//! number of faulty DECLARATIONS.
//!
//! ## Bounds
//!
//! One pass over the `Resources` section of an already bounded document
//! ([`super::super::template::parse`] refuses an over-long body before anything here sees
//! it), and the faults are collected one per declaration, so the diagnostic is bounded by the
//! resource quota the same way the expansion is.

use super::*;

/// The namespace the macro's own input types live in: a type here that the macro does not
/// define is a type nothing recognises.
const SERVERLESS_NAMESPACE: &str = "AWS::Serverless::";

/// The types the macro DEFINES, as the AWS SAM resource reference lists them. The seven
/// [`super`] expands, plus the three it leaves as declared — an application resolved from the
/// AWS Serverless Application Repository, a connector, and a GraphQL API — which are
/// declarations the macro accepts even where this emulator expands nothing for them.
const SERVERLESS_TYPES: &[&str] = &[
    FUNCTION,
    API,
    HTTP_API,
    WEBSOCKET_API,
    SIMPLE_TABLE,
    LAYER_VERSION,
    STATE_MACHINE,
    "AWS::Serverless::Application",
    CONNECTOR,
    "AWS::Serverless::GraphQLApi",
];

/// The properties the macro cannot expand a declaration PAST being without, per type: the
/// members the resource a declaration becomes is created from that have no value the expansion
/// could supply, so a declaration stating none describes no resource at all.
///
/// One entry today: an `AWS::Serverless::WebSocketApi` becomes a WEBSOCKET
/// `AWS::ApiGatewayV2::Api`, and `CreateApi$RouteSelectionExpression` — "the route selection
/// expression for the API … This property is required for WebSocket APIs" — is the member that
/// api could not be created without, there being no default for a protocol whose routes are
/// matched against a message. Every other property of every type the macro expands either has
/// a documented default, is supplied by the expansion itself, or is judged by the service that
/// owns the resource it lands on, which is where the real macro leaves it too.
const REQUIRED_PROPERTIES: &[(&str, &[&str])] = &[(WEBSOCKET_API, &[ROUTE_SELECTION_EXPRESSION])];

/// The `PackageType` a function whose code is a deployment package carries, which is also the
/// default: "Zip".
const ZIP_PACKAGE_TYPE: &str = "Zip";

/// What a `Zip` function with no runtime or no handler is invalid for, in the validator's own
/// words. Entirely fixed text.
const ZIP_NEEDS_RUNTIME_AND_HANDLER: &str =
    "Runtime and Handler needs to be present when PackageType is of type `Zip`";

/// What the validator reports for a `SimpleTable` whose stated `PrimaryKey` says nothing
/// about its type. Fixed text, naming the property path it is about.
const PRIMARY_KEY_TYPE_REQUIRED: &str = "Property 'PrimaryKey.Type' is required.";

/// How a value the validator reports no event type for is spelled in its diagnostic — an
/// `Events` entry that states none at all.
const NO_EVENT_TYPE: &str = "None";

/// The event source types an `Events` entry may name, as the AWS SAM "Event source types"
/// reference lists them. [`super`] synthesizes a resource for the ones it expands
/// ([`super::EVENT_SOURCE_MAPPINGS`] and the rules and subscriptions beside them) and leaves
/// the rest to the resource each rewrites, but EVERY name here is one the macro accepts —
/// which is what this roster is for.
const EVENT_TYPES: &[&str] = &[
    "AlexaSkill",
    "Api",
    "CloudWatchEvent",
    "CloudWatchLogs",
    "Cognito",
    "DocumentDB",
    "DynamoDB",
    "EventBridgeRule",
    "HttpApi",
    "IoTRule",
    "Kinesis",
    "MQ",
    "MSK",
    "S3",
    "Schedule",
    "ScheduleV2",
    "SelfManagedKafka",
    "SNS",
    "SQS",
];

/// The `Policies` property, named in the diagnostic of an entry that is not a policy.
const POLICIES: &str = "Policies";

/// `Some(reason)` for a document the Serverless macro cannot process, worded as the failure
/// the operation that submitted it records; `None` for one it can.
///
/// The document is the one as SUBMITTED — the macro validates its own input — and it is asked
/// only of a document that DECLARES the transform, which is what makes this the macro's
/// verdict rather than a rule about templates in general.
pub(super) fn failure(document: &Value) -> Option<String> {
    if !declares_the_transform(document) {
        return None;
    }
    let resources = document.get("Resources").and_then(Value::as_object)?;
    // CloudFormation's own refusal comes first: a document declaring a type that does not
    // exist is one no macro is asked about.
    let unrecognised = unrecognised_types(resources);
    if !unrecognised.is_empty() {
        return Some(crate::state::resource_types::unrecognized_message(
            &unrecognised,
        ));
    }
    let globals = document.get("Globals").cloned().unwrap_or(Value::Null);
    let causes: Vec<String> = resources
        .iter()
        .filter_map(|(logical_id, declaration)| {
            fault_in(declaration, &globals, resources)
                .map(|detail| format!("Resource with id [{logical_id}] is invalid. {detail}"))
        })
        .collect();
    if causes.is_empty() {
        return None;
    }
    Some(format!(
        "Transform {TRANSFORM} failed with: Invalid Serverless Application Specification \
         document. Number of errors found: {}. {}",
        causes.len(),
        causes.join(" ")
    ))
}

/// The types the document declares in the Serverless namespace that the macro does not
/// define, in declaration order and without repeats.
fn unrecognised_types(resources: &Map<String, Value>) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for declaration in resources.values() {
        let Some(declared) = resource_type(declaration) else {
            continue;
        };
        if !declared.starts_with(SERVERLESS_NAMESPACE)
            || SERVERLESS_TYPES.contains(&declared)
            || found.iter().any(|held| held == declared)
        {
            continue;
        }
        found.push(declared.to_string());
    }
    found
}

/// What ONE declaration is invalid for, or `None` for one the macro can expand.
///
/// A property the declaration's type cannot be expanded without is weighed first
/// ([`missing_property_fault`]), because the resource is built from its properties before
/// anything else about it is judged.
///
/// The properties are read with `Globals` resolved into them, exactly as the expansion reads
/// them ([`super::expanded_resources`]): a runtime or a handler the template states once for
/// every function is stated for each of them, so a declaration is judged on what it really
/// carries.
///
/// A declaration judged against OTHER declarations of the same document — a connector, whose
/// ends are resources of it — is handed the whole `Resources` section for that reason.
fn fault_in(
    declaration: &Value,
    globals: &Value,
    resources: &Map<String, Value>,
) -> Option<String> {
    let declared = resource_type(declaration)?;
    // The resource is BUILT from its properties before anything else about it is judged, so a
    // property it could not be built without is the first fault reported for it.
    if let Some(fault) = missing_property_fault(declared, declaration.get("Properties")) {
        return Some(fault);
    }
    match declared {
        FUNCTION => function_fault(&merged(
            globals.get("Function"),
            declaration.get("Properties"),
            0,
        )),
        API => api_fault(&merged(
            globals.get("Api"),
            declaration.get("Properties"),
            0,
        )),
        SIMPLE_TABLE => table_fault(&merged(
            globals.get("SimpleTable"),
            declaration.get("Properties"),
            0,
        )),
        // A state machine takes the same two properties a function does — the policies the
        // role generated for it grants, and the events it is triggered by — so the same two
        // rules are what the macro cannot expand past for it. `Globals` has no section for
        // this type, so its own declaration is the whole of what it states.
        STATE_MACHINE => {
            let properties = declaration.get("Properties").cloned().unwrap_or(Value::Null);
            policies_fault(&properties).or_else(|| events_fault(&properties))
        }
        // A connector is judged on the permissions the profile of the pair it runs between
        // publishes, which is the one thing about it the macro cannot expand past
        // ([`connector`]). `Globals` has no section for this type.
        CONNECTOR => connector::fault(declaration, resources),
        _ => None,
    }
}

/// What one `AWS::Serverless::Function` declaration is invalid for.
///
/// The order is the validator's own: the function itself is built first — which is where its
/// package type, runtime and handler are judged — then the role its `Policies` grant, then the
/// resources its events are expanded into.
fn function_fault(properties: &Value) -> Option<String> {
    package_fault(properties)
        .or_else(|| policies_fault(properties))
        .or_else(|| events_fault(properties))
}

/// `Some(detail)` for a function whose code is a deployment package and that states no runtime
/// to run it on, or no handler to enter it at: "Runtime and Handler are required if
/// PackageType is set to Zip", which is also the default package type.
///
/// A function whose `PackageType` is anything else carries its entry point in the image, so
/// neither property is required of it.
fn package_fault(properties: &Value) -> Option<String> {
    let package_type = properties
        .get("PackageType")
        .and_then(Value::as_str)
        .unwrap_or(ZIP_PACKAGE_TYPE);
    if package_type != ZIP_PACKAGE_TYPE {
        return None;
    }
    let stated = |property: &str| {
        properties
            .get(property)
            .is_some_and(|value| !value.is_null())
    };
    (!stated("Runtime") || !stated("Handler"))
        .then(|| ZIP_NEEDS_RUNTIME_AND_HANDLER.to_string())
}

/// `Some(detail)` for the FIRST `Policies` entry that is none of the three things the property
/// accepts, naming the position it is at — which is how the validator reports it, because an
/// entry's position is all that identifies it.
fn policies_fault(properties: &Value) -> Option<String> {
    declared_policies(properties.get(POLICIES))
        .iter()
        .position(|entry| !is_a_policy(entry))
        .map(|index| format!("Policy at index {index} in the '{POLICIES}' property is not valid"))
}

/// Whether one `Policies` entry is something the property accepts: "a single value or a list
/// of values, where each value can be the name of an AWS managed policy, an inline IAM policy
/// document, or an AWS SAM policy template".
///
/// A STRING names a managed policy — by name or by ARN — and nothing here decides whether that
/// policy exists: an entry naming one IAM does not hold is a role IAM refuses, which is where
/// the real expansion leaves it too. An OBJECT is an inline policy document when it carries a
/// `Statement`, a policy template when its one member names a published template
/// ([`super::policy_templates`]), and an intrinsic function where the template computes the
/// entry — CloudFormation resolves that as it deploys. Anything else is an entry the macro can
/// make nothing of.
fn is_a_policy(entry: &Value) -> bool {
    match entry {
        Value::String(_) => true,
        Value::Object(members) if members.contains_key("Statement") => true,
        Value::Object(members) => {
            // The `Ref`/`Fn::<name>` reading is the crate's one
            // ([`crate::state::template::is_intrinsic`]); the CONDITION reference is the other
            // value form CloudFormation resolves, which an entry may equally be computed by.
            crate::state::template::is_intrinsic(members)
                || names_only(members, CONDITION_REFERENCE)
                || names_a_policy_template(members)
        }
        _ => false,
    }
}

/// The member a value that is a CONDITION's verdict carries.
const CONDITION_REFERENCE: &str = "Condition";

/// Whether an object has exactly one member, and it is `name`.
fn names_only(members: &Map<String, Value>, name: &str) -> bool {
    let mut entries = members.keys();
    entries.next().is_some_and(|held| held == name) && entries.next().is_none()
}

/// Whether an object APPLIES a published policy template: one member, whose name is a template
/// the catalogue names ([`super::policy_templates::is_published`]) and whose value states that
/// template's parameters.
fn names_a_policy_template(members: &Map<String, Value>) -> bool {
    let mut entries = members.keys();
    entries
        .next()
        .is_some_and(|name| policy_templates::is_published(name))
        && entries.next().is_none()
}

/// `Some(detail)` for the FIRST `Events` entry that names no event source type, naming the
/// entry by the id the declaration gives it and reporting the type it did state — the entry
/// is the macro's input, and an event it cannot place is one it can expand nothing for.
fn events_fault(properties: &Value) -> Option<String> {
    object(properties.get("Events"))
        .iter()
        .find(|(_, declaration)| {
            !declaration
                .get("Type")
                .and_then(Value::as_str)
                .is_some_and(|stated| EVENT_TYPES.contains(&stated))
        })
        .map(|(event, declaration)| {
            let stated = declaration
                .get("Type")
                .and_then(Value::as_str)
                .unwrap_or(NO_EVENT_TYPE);
            format!(
                "Event with id [{event}] is invalid. Resource dict has missing or invalid value \
                 for key Type. Event Type is: {stated}."
            )
        })
}

/// What one `AWS::Serverless::Api` declaration is invalid for.
///
/// The order is the validator's own: the api's AUTHORIZERS are built before the default
/// authorizer is set over them, so an authorizer that could never authorize a request is
/// reported before a default that names an authorizer the api does not declare.
fn api_fault(properties: &Value) -> Option<String> {
    let auth = object(properties.get(auth::AUTH_PROPERTY));
    let authorizers = object(auth.get(auth::AUTHORIZERS));
    if let Some(fault) = authorizers
        .iter()
        .find_map(|(name, declaration)| authorizer_fault(name, declaration))
    {
        return Some(fault);
    }
    let default = auth.get(auth::DEFAULT_AUTHORIZER).and_then(Value::as_str)?;
    if matches!(default, auth::AWS_IAM | auth::NO_AUTHORIZER) || authorizers.contains_key(default) {
        return None;
    }
    Some(format!(
        "Unable to set DefaultAuthorizer because '{default}' was not defined in \
         '{}'",
        auth::AUTHORIZERS
    ))
}

/// `Some(detail)` for one declared authorizer that could never authorize a request: a Lambda
/// authorizer called with the WHOLE request that names no part of the request to authorize
/// on.
///
/// The rule is API Gateway's own, stated over the authorizer that service would create:
/// "`identitySource` … For the `REQUEST` authorizer, this is required when authorization
/// caching is enabled", and caching is enabled exactly while the result TTL is above zero —
/// which an authorizer that states no `ReauthorizeEvery` has, since that service picks its
/// own default TTL for a create that sets none. So an authorizer that caches its results has
/// to say what to cache them BY, and one with caching switched off may authorize on the
/// request alone.
///
/// A `TOKEN` authorizer reads its token from a header, and one that names none reads the
/// `Authorization` header, so there is no such thing as a token authorizer with no identity;
/// a user-pool authorizer is in the same position.
fn authorizer_fault(name: &str, declaration: &Value) -> Option<String> {
    if declaration.get(auth::USER_POOL_ARN).is_some() {
        return None;
    }
    declaration.get(auth::FUNCTION_ARN)?;
    if declaration
        .get(auth::FUNCTION_PAYLOAD_TYPE)
        .and_then(Value::as_str)
        != Some(auth::PAYLOAD_TYPE_REQUEST)
    {
        return None;
    }
    let identity = declaration.get(auth::IDENTITY);
    if identity
        .and_then(|identity| identity.get(auth::IDENTITY_REAUTHORIZE_EVERY))
        .and_then(Value::as_i64)
        == Some(CACHING_DISABLED)
    {
        return None;
    }
    if !auth::request_identity_sources(identity).is_empty() {
        return None;
    }
    Some(format!(
        "{name} Authorizer must specify Identity with at least one of {}.",
        identity_sources_as_listed()
    ))
}

/// The value an authorizer's `Identity` switches result caching OFF with: "if it equals 0,
/// authorization caching is disabled" (`Authorizer$authorizerResultTtlInSeconds`, which the
/// member the declaration states it under becomes — [`auth::IDENTITY_REAUTHORIZE_EVERY`]).
const CACHING_DISABLED: i64 = 0;

/// The identity sources the sentence above lists, in the order the property publishes them
/// ([`auth::REQUEST_IDENTITY_SOURCES`]), with the last joined by `or` as the validator words
/// it.
fn identity_sources_as_listed() -> String {
    let names: Vec<&str> = auth::REQUEST_IDENTITY_SOURCES
        .iter()
        .map(|(property, _)| *property)
        .collect();
    match names.split_last() {
        Some((last, before)) => format!("{}, or {last}", before.join(", ")),
        None => String::new(),
    }
}

/// `Some(detail)` for one `AWS::Serverless::SimpleTable` whose stated `PrimaryKey` says
/// nothing about its type.
///
/// The property is optional and "Type … Default: String" applies to the key the macro
/// SYNTHESIZES — a declaration that states a primary key of its own states its type with it,
/// and one that states neither gets the default key whole.
fn table_fault(properties: &Value) -> Option<String> {
    let key = properties.get("PrimaryKey")?;
    if key.is_null() {
        return None;
    }
    key.get("Type")
        .filter(|stated| !stated.is_null())
        .is_none()
        .then(|| PRIMARY_KEY_TYPE_REQUIRED.to_string())
}

/// `Some(detail)` for a declaration that states none of a property its type cannot be expanded
/// without, in the words the validator reports a missing property in: `Missing required
/// property '<name>'.`, naming the member the SAM schema requires.
///
/// The properties are read off the declaration ITSELF, since no type in the roster has a
/// `Globals` section to inherit one from ("properties that you specify in the Globals section"
/// are published for functions, APIs and simple tables).
///
/// A property stated as `null` states nothing, so it reads as an absent one — the same reading
/// every other rule here takes of a null property.
fn missing_property_fault(resource_type: &str, properties: Option<&Value>) -> Option<String> {
    let required = REQUIRED_PROPERTIES
        .iter()
        .find(|(declared, _)| *declared == resource_type)
        .map(|(_, properties)| *properties)?;
    required
        .iter()
        .find(|property| {
            properties
                .and_then(|properties| properties.get(**property))
                .is_none_or(Value::is_null)
        })
        .map(|property| format!("Missing required property '{property}'."))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document the macro can process carries no failure, and each class of fault is
    /// reported as the validator words it — counted, and naming the declaration it is about.
    ///
    /// The rows differ along what the rule generalises over: WHICH fault the document carries,
    /// HOW MANY declarations are faulty, and whether the fault is the macro's own or
    /// CloudFormation's.
    #[test]
    fn a_faulty_document_is_reported_fault_by_fault() {
        let valid = json!({
            "Transform": TRANSFORM,
            "Resources": {
                "Fn": {
                    "Type": FUNCTION,
                    "Properties": {
                        "Runtime": "python3.12",
                        "Handler": "index.handler",
                        "InlineCode": "def handler(event, context): pass",
                        "Policies": [
                            "AWSLambdaBasicExecutionRole",
                            {"DynamoDBCrudPolicy": {"TableName": "items"}},
                            {"Statement": [{"Effect": "Allow", "Action": "s3:*", "Resource": "*"}]},
                            {"Ref": "APolicyParameter"},
                        ],
                        "Events": {"Ev": {"Type": "Api", "Properties": {"Path": "/", "Method": "get"}}},
                    },
                },
            },
        });
        assert_eq!(
            failure(&valid),
            None,
            "case \"a document the macro processes\": expected no failure, got {:?}",
            failure(&valid)
        );

        // (case, the document, the reason it fails with)
        let rows: Vec<(&str, Value, String)> = vec![
            (
                "a Policies entry naming no published template",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {"Fn": {"Type": FUNCTION, "Properties": {
                        "Runtime": "python3.12",
                        "Handler": "index.handler",
                        "Policies": [{"NotAPolicyTemplate": {"TableName": "items"}}],
                    }}},
                }),
                wrapped(1, &["Resource with id [Fn] is invalid. Policy at index 0 in the \
                              'Policies' property is not valid"]),
            ),
            (
                "a Zip function with no handler",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {"Fn": {"Type": FUNCTION, "Properties": {
                        "Runtime": "python3.12",
                    }}},
                }),
                wrapped(
                    1,
                    &["Resource with id [Fn] is invalid. Runtime and Handler needs to be \
                       present when PackageType is of type `Zip`"],
                ),
            ),
            (
                "an event naming no event source type",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {"Fn": {"Type": FUNCTION, "Properties": {
                        "Runtime": "python3.12",
                        "Handler": "index.handler",
                        "Events": {"Ev": {"Type": "NotAnEventType"}},
                    }}},
                }),
                wrapped(
                    1,
                    &["Resource with id [Fn] is invalid. Event with id [Ev] is invalid. \
                       Resource dict has missing or invalid value for key Type. Event Type \
                       is: NotAnEventType."],
                ),
            ),
            (
                "a state machine whose event names no event source type",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {"Machine": {"Type": STATE_MACHINE, "Properties": {
                        "Definition": {"StartAt": "Done", "States": {"Done": {"Type": "Succeed"}}},
                        "Events": {"Ev": {"Type": "NotAnEventType"}},
                    }}},
                }),
                wrapped(
                    1,
                    &["Resource with id [Machine] is invalid. Event with id [Ev] is invalid. \
                       Resource dict has missing or invalid value for key Type. Event Type \
                       is: NotAnEventType."],
                ),
            ),
            (
                "a simple table whose primary key states no type",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {"Table": {"Type": SIMPLE_TABLE, "Properties": {
                        "PrimaryKey": {"Name": "id"},
                    }}},
                }),
                wrapped(
                    1,
                    &["Resource with id [Table] is invalid. Property 'PrimaryKey.Type' is \
                       required."],
                ),
            ),
            (
                "two faulty declarations",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {
                        "Fn": {"Type": FUNCTION, "Properties": {"Runtime": "python3.12"}},
                        "Table": {"Type": SIMPLE_TABLE, "Properties": {
                            "PrimaryKey": {"Name": "id"},
                        }},
                    },
                }),
                wrapped(
                    2,
                    &[
                        "Resource with id [Fn] is invalid. Runtime and Handler needs to be \
                         present when PackageType is of type `Zip`",
                        "Resource with id [Table] is invalid. Property 'PrimaryKey.Type' is \
                         required.",
                    ],
                ),
            ),
            (
                "a REQUEST authorizer that names no part of the request to authorize on",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {"Web": {"Type": API, "Properties": {
                        "StageName": "live",
                        "Auth": {"Authorizers": {"Gate": {
                            "FunctionArn": {"Fn::GetAtt": ["Guard", "Arn"]},
                            "FunctionPayloadType": "REQUEST",
                        }}},
                    }}},
                }),
                wrapped(
                    1,
                    &["Resource with id [Web] is invalid. Gate Authorizer must specify \
                       Identity with at least one of Headers, QueryStrings, StageVariables, \
                       or Context."],
                ),
            ),
            (
                "a default authorizer the api does not declare",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {"Web": {"Type": API, "Properties": {
                        "StageName": "live",
                        "Auth": {"DefaultAuthorizer": "Missing"},
                    }}},
                }),
                wrapped(
                    1,
                    &["Resource with id [Web] is invalid. Unable to set DefaultAuthorizer \
                       because 'Missing' was not defined in 'Authorizers'"],
                ),
            ),
            (
                "a connector asking a permission its pair does not publish",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {
                        "Fn": {"Type": FUNCTION, "Properties": {
                            "Runtime": "python3.12",
                            "Handler": "index.handler",
                        }},
                        "Table": {"Type": SIMPLE_TABLE, "Properties": {}},
                        "Connector": {"Type": CONNECTOR, "Properties": {
                            "Source": {"Id": "Fn"},
                            "Destination": {"Id": "Table"},
                            "Permissions": ["Delete"],
                        }},
                    },
                }),
                wrapped(
                    1,
                    &["Resource with id [Connector] is invalid. Unsupported 'Permissions' \
                       provided for connector from AWS::Lambda::Function to \
                       AWS::DynamoDB::Table; valid values are: Read, Write."],
                ),
            ),
            (
                "a type the macro does not define",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {"Thing": {"Type": "AWS::Serverless::NotAThing"}},
                }),
                "Template format error: Unrecognized resource types: \
                 [AWS::Serverless::NotAThing]"
                    .to_string(),
            ),
        ];

        for (case, document, expected) in rows {
            let answered = failure(&document);
            assert_eq!(
                answered.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {answered:?}"
            );
        }
    }

    /// A function whose runtime and handler come from `Globals` states them, and so does a
    /// function whose code is an IMAGE rather than a package — neither is a fault.
    #[test]
    fn a_function_states_its_entry_point_wherever_the_template_states_it() {
        // (case, the document)
        let rows: Vec<(&str, Value)> = vec![
            (
                "a runtime and a handler stated once for every function",
                json!({
                    "Transform": TRANSFORM,
                    "Globals": {"Function": {"Runtime": "python3.12", "Handler": "index.handler"}},
                    "Resources": {"Fn": {"Type": FUNCTION, "Properties": {"MemorySize": 512}}},
                }),
            ),
            (
                "a function whose code is an image",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {"Fn": {"Type": FUNCTION, "Properties": {
                        "PackageType": "Image",
                        "ImageUri": "000000000000.dkr.ecr.us-east-1.amazonaws.com/app:v1",
                    }}},
                }),
            ),
            (
                "a simple table that states no primary key at all",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {"Table": {"Type": SIMPLE_TABLE, "Properties": {}}},
                }),
            ),
            (
                "a document declaring no Serverless resource under the transform",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {"Queue": {"Type": "AWS::SQS::Queue", "Properties": {}}},
                }),
            ),
            (
                "a document that declares the transform nowhere",
                json!({
                    "Resources": {"Fn": {"Type": FUNCTION, "Properties": {}}},
                }),
            ),
        ];

        for (case, document) in rows {
            let answered = failure(&document);
            assert_eq!(
                answered, None,
                "case {case:?}: expected no failure, got {answered:?}"
            );
        }
    }

    /// RULE: an api whose `Auth` states an authorizer that CAN authorize a request, and a
    /// default the api declares, is a document the macro processes.
    ///
    /// The rows differ along what the rule generalises over: which kind of authorizer the
    /// declaration states (a Lambda authorizer called with the whole request, one called with
    /// a token, a user-pool authorizer), where a `REQUEST` authorizer's identity comes from
    /// (a request part it names, or caching switched off so it needs none) and what the
    /// default authorizer is (a declared authorizer, or the caller's own credential, which the
    /// api declares nothing for).
    #[test]
    fn an_api_whose_auth_can_authorize_a_request_is_not_a_fault() {
        // (case, the document)
        let rows: Vec<(&str, Value)> = vec![
            (
                "a REQUEST authorizer naming a header to authorize on",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {"Web": {"Type": API, "Properties": {
                        "StageName": "live",
                        "Auth": {"Authorizers": {"Gate": {
                            "FunctionArn": {"Fn::GetAtt": ["Guard", "Arn"]},
                            "FunctionPayloadType": "REQUEST",
                            "Identity": {"Headers": ["Authorization"]},
                        }}},
                    }}},
                }),
            ),
            (
                "a REQUEST authorizer naming a context value to authorize on",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {"Web": {"Type": API, "Properties": {
                        "Auth": {"Authorizers": {"Gate": {
                            "FunctionArn": {"Fn::GetAtt": ["Guard", "Arn"]},
                            "FunctionPayloadType": "REQUEST",
                            "Identity": {"Context": ["tenant"]},
                        }}},
                    }}},
                }),
            ),
            (
                "a REQUEST authorizer with caching switched off, which needs no identity",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {"Web": {"Type": API, "Properties": {
                        "Auth": {"Authorizers": {"Gate": {
                            "FunctionArn": {"Fn::GetAtt": ["Guard", "Arn"]},
                            "FunctionPayloadType": "REQUEST",
                            "Identity": {"ReauthorizeEvery": 0},
                        }}},
                    }}},
                }),
            ),
            (
                "a TOKEN authorizer, which reads its token from a header either way",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {"Web": {"Type": API, "Properties": {
                        "Auth": {
                            "DefaultAuthorizer": "Gate",
                            "Authorizers": {"Gate": {
                                "FunctionArn": {"Fn::GetAtt": ["Guard", "Arn"]},
                            }},
                        },
                    }}},
                }),
            ),
            (
                "a user-pool authorizer set as the default",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {"Web": {"Type": API, "Properties": {
                        "Auth": {
                            "DefaultAuthorizer": "Pool",
                            "Authorizers": {"Pool": {"UserPoolArn": {"Fn::GetAtt": ["Users", "Arn"]}}},
                        },
                    }}},
                }),
            ),
            (
                "the caller's own credential as the default authorizer",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {"Web": {"Type": API, "Properties": {
                        "Auth": {"DefaultAuthorizer": "AWS_IAM"},
                    }}},
                }),
            ),
            (
                "an api that states no authorization at all",
                json!({
                    "Transform": TRANSFORM,
                    "Resources": {"Web": {"Type": API, "Properties": {"StageName": "live"}}},
                }),
            ),
        ];

        for (case, document) in rows {
            let answered = failure(&document);
            assert_eq!(
                answered, None,
                "case {case:?}: expected no failure, got {answered:?}"
            );
        }
    }

    /// The diagnostic one fault count and one set of causes reads as, which is the wrapper the
    /// rows above are written against.
    fn wrapped(count: usize, causes: &[&str]) -> String {
        format!(
            "Transform {TRANSFORM} failed with: Invalid Serverless Application Specification \
             document. Number of errors found: {count}. {}",
            causes.join(" ")
        )
    }
}
