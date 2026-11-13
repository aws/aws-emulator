// SPDX-License-Identifier: CC0-1.0

//! `AWS::AmplifyUIBuilder::Component`, `::Form` and `::Theme` are resource types a template may
//! DECLARE: every operation that submits a template admits a body declaring them, and the
//! catalogue entries behind that say what a resource of each type reports.
//!
//! The rule under test: **a resource type Amazon Web Services publishes belongs to the
//! catalogue a submitted template is held to, so a body declaring it is admitted and the type
//! reports the identity and the return values its own contract publishes** — the half of the
//! rule this crate owns on its own. It is the mirror of `unrecognized_resource_types.rs`, which
//! pins the other direction: a type CloudFormation does NOT know is refused `Template format
//! error: Unrecognized resource types: [<types>]`, and that refusal must never be the answer
//! for a type AWS publishes.
//!
//! What the stack then DOES with such a declaration — the call to Amplify UI Builder's own
//! `CreateComponent`/`CreateForm`/`CreateTheme` against the app AWS Amplify holds, the id that
//! service mints, the change an update applies and the entity's removal with the stack — needs
//! both of those services mounted beside this one, so it lives in the router's own cross-service
//! suite (`core/router/tests/all/cloudformation_provisions_amplifyuibuilder_entities.rs`). This
//! suite asserts admission alone, which is what a client sees before any resource is attempted.
//!
//! The rows differ along what the rule generalises over: WHICH of the three types the body
//! declares, the declared properties (the required members alone, those plus the optional ones
//! each type publishes, those plus the type's own `Tags`), where the app id comes from (the app
//! resource the same body declares, and a literal), the template spelling (JSON and YAML), how
//! many entities one body declares, and whether the declaration stands beside a resource of
//! another type.

use service_com_amazonaws_cloudformation_cloudformation::state::resource_types::{
    is_published, is_recognized, AWS_RESOURCE_TYPES,
};
use service_com_amazonaws_cloudformation_cloudformation::state::template::{
    identity_is_service_assigned, name_property_candidates, published_attribute_names,
    replacement_properties, AMPLIFYUIBUILDER_COMPONENT, AMPLIFYUIBUILDER_FORM,
    AMPLIFYUIBUILDER_THEME, AMPLIFY_APP,
};

use crate::stack_lifecycle::{call, create_stack, member, router, xml_text};

/// The fixed form of the refusal a type CloudFormation does not know is answered with, which no
/// row here may ever see.
const UNRECOGNIZED_PREFIX: &str = "Template format error: Unrecognized resource types: [";

/// The app environment the entities below live in: an Amplify app id of the form that service's
/// own `AppId` member fixes (`^d[a-z0-9]+$`), and the environment Amplify Studio names.
const APP_ID: &str = "d1234abcdefghij";
const ENVIRONMENT: &str = "staging";

/// One row: a template body declaring one or more of the three types, and the name of the stack
/// it is submitted under.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: String,
}

fn rows() -> Vec<Row> {
    vec![
        // The reported case: the three types in one body, in the app the body itself declares,
        // with every published return value read back through `Outputs`.
        Row {
            case: "a component, a form and a theme in the app the body declares",
            stack_name: "amplifyuibuilder-admitted",
            template: format!(
                r#"{{"AWSTemplateFormatVersion": "2010-09-09",
                     "Resources": {{
                       "App": {{"Type": "{AMPLIFY_APP}",
                         "Properties": {{"Name": "admitted-app"}}}},
                       "Theme": {{"Type": "{AMPLIFYUIBUILDER_THEME}",
                         "Properties": {{"AppId": {{"Fn::GetAtt": ["App", "AppId"]}},
                           "EnvironmentName": "{ENVIRONMENT}", "Name": "AdmittedTheme",
                           "Values": [{{"Key": "tokens", "Value": {{"Value": "small"}}}}]}}}},
                       "Form": {{"Type": "{AMPLIFYUIBUILDER_FORM}",
                         "Properties": {{"AppId": {{"Fn::GetAtt": ["App", "AppId"]}},
                           "EnvironmentName": "{ENVIRONMENT}", "Name": "AdmittedForm",
                           "DataType": {{"DataSourceType": "Custom", "DataTypeName": "Todo"}},
                           "FormActionType": "create", "SchemaVersion": "1.0",
                           "Fields": {{}}, "SectionalElements": {{}}, "Style": {{}}}}}},
                       "Component": {{"Type": "{AMPLIFYUIBUILDER_COMPONENT}",
                         "Properties": {{"AppId": {{"Fn::GetAtt": ["App", "AppId"]}},
                           "EnvironmentName": "{ENVIRONMENT}", "Name": "AdmittedComponent",
                           "ComponentType": "Button", "Properties": {{}}, "Overrides": {{}},
                           "BindingProperties": {{}}, "Variants": []}}}}}},
                     "Outputs": {{
                       "ThemeRef": {{"Value": {{"Ref": "Theme"}}}},
                       "ThemeId": {{"Value": {{"Fn::GetAtt": ["Theme", "Id"]}}}},
                       "ThemeCreatedAt": {{"Value": {{"Fn::GetAtt": ["Theme", "CreatedAt"]}}}},
                       "ThemeModifiedAt": {{"Value": {{"Fn::GetAtt": ["Theme", "ModifiedAt"]}}}},
                       "FormAppId": {{"Value": {{"Fn::GetAtt": ["Form", "AppId"]}}}},
                       "FormEnvironment": {{
                         "Value": {{"Fn::GetAtt": ["Form", "EnvironmentName"]}}}},
                       "ComponentId": {{"Value": {{"Fn::GetAtt": ["Component", "Id"]}}}}}}}}"#
            ),
        },
        // One component carrying the optional members the type publishes — its children, its
        // events, its variants and its collection properties — against a LITERAL app id.
        Row {
            case: "a component carrying its children, events, variants and collection data",
            stack_name: "amplifyuibuilder-component",
            template: format!(
                r#"{{"Resources": {{"Component": {{"Type": "{AMPLIFYUIBUILDER_COMPONENT}",
                     "Properties": {{"AppId": "{APP_ID}", "EnvironmentName": "{ENVIRONMENT}",
                       "Name": "RichComponent", "ComponentType": "Collection",
                       "SchemaVersion": "1.0", "SourceId": "figma-1",
                       "Properties": {{"label": {{"Value": "Save"}}}},
                       "Overrides": {{"Button": {{"fontWeight": "bold"}}}},
                       "BindingProperties": {{"item": {{"Type": "Data"}}}},
                       "CollectionProperties": {{"items": {{"Model": "Todo"}}}},
                       "Variants": [{{"VariantValues": {{"breakpoint": "large"}},
                         "Overrides": {{"Button": {{"fontSize": "20px"}}}}}}],
                       "Children": [{{"ComponentType": "Text", "Name": "Caption",
                         "Properties": {{"label": {{"Value": "Saves it"}}}}}}],
                       "Events": {{"onClick": {{"Action": "Amplify.Navigation",
                         "Parameters": {{"Anchor": {{"Value": "top"}}}}}}}},
                       "Tags": {{"owner": "studio"}}}}}}}}}}"#
            ),
        },
        // The YAML spelling, with a form carrying its call-to-action and sectional elements and
        // the type's own `Tags`, beside a resource of another type: the catalogue admits every
        // type a body declares or none of them.
        Row {
            case: "a form in YAML beside a resource of another type",
            stack_name: "amplifyuibuilder-yaml",
            template: format!(
                "\
Resources:
  Form:
    Type: {AMPLIFYUIBUILDER_FORM}
    Properties:
      AppId: {APP_ID}
      EnvironmentName: {ENVIRONMENT}
      Name: YamlForm
      DataType:
        DataSourceType: DataStore
        DataTypeName: Todo
      FormActionType: update
      SchemaVersion: \"1.0\"
      LabelDecorator: required
      Fields:
        title:
          Label: Title
          InputType:
            Type: TextField
            Required: true
      SectionalElements:
        Heading:
          Type: Heading
          Level: 1
          Text: Todo
      Style:
        HorizontalGap:
          Value: 16px
      Cta:
        Position: bottom
        Submit:
          Excluded: false
      Tags:
        owner: studio
  Topic:
    Type: AWS::SNS::Topic
    Properties:
      TopicName: amplifyuibuilder-yaml-topic
Outputs:
  FormId:
    Value: !GetAtt Form.Id
"
            ),
        },
        // Two themes of one body, each in its own app environment: a type's declarations are
        // admitted however many of them one body carries.
        Row {
            case: "two themes in two app environments",
            stack_name: "amplifyuibuilder-pair",
            template: format!(
                r#"{{"Resources": {{
                     "First": {{"Type": "{AMPLIFYUIBUILDER_THEME}",
                       "Properties": {{"AppId": "{APP_ID}", "EnvironmentName": "{ENVIRONMENT}",
                         "Name": "FirstTheme",
                         "Values": [{{"Key": "tokens", "Value": {{"Children": [
                           {{"Key": "brand", "Value": {{"Value": "teal-80"}}}}]}}}}]}}}},
                     "Second": {{"Type": "{AMPLIFYUIBUILDER_THEME}",
                       "Properties": {{"AppId": "{APP_ID}", "EnvironmentName": "dev",
                         "Name": "SecondTheme", "Values": [],
                         "Overrides": [{{"Key": "dark", "Value": {{"Value": "white"}}}}]}}}}}}}}"#
            ),
        },
    ]
}

/// Assert one answer admitted the body rather than refusing its resource types.
fn assert_admitted(case: &str, status: u16, body: &str) {
    let message = xml_text(body, "Message").unwrap_or_default();
    assert!(
        !message.starts_with(UNRECOGNIZED_PREFIX),
        "case {case:?}: expected a type Amazon Web Services publishes to be recognised, got \
         {message:?}"
    );
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for a body declaring a published type, got {status} \
         with body {body}"
    );
}

/// Every operation that submits a template for deployment admits a body declaring the types, and
/// so does the operation whose whole answer is a verdict on the body.
#[tokio::test]
async fn every_operation_that_takes_a_template_admits_the_amplifyuibuilder_types() {
    for row in rows() {
        let router = router();
        let (status, body) = create_stack(&router, row.stack_name, &row.template, &[]).await;
        assert_admitted(row.case, status, &body);
        let stack_id = xml_text(&body, "StackId").unwrap_or_default();
        assert!(
            stack_id.contains(":stack/"),
            "case {:?}: expected CreateStack to answer a stack id, got {body}",
            row.case
        );

        let (status, body) = call(
            &router,
            "ValidateTemplate",
            &[member("TemplateBody", &row.template)],
        )
        .await;
        assert_admitted(row.case, status, &body);

        let (status, body) = call(
            &router,
            "CreateChangeSet",
            &[
                member("StackName", &format!("{}-change-set", row.stack_name)),
                member("ChangeSetName", "a-change-set"),
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", &row.template),
            ],
        )
        .await;
        assert_admitted(row.case, status, &body);
    }
}

/// The catalogue entries the rule rests on, type by type: each is published and recognised, a
/// near-miss spelling of it still is not, and each reports the identity and the return values
/// its own contract publishes.
#[test]
fn the_catalogue_publishes_the_amplifyuibuilder_types_and_what_they_return() {
    // (the type, the attributes it publishes)
    let rows: [(&str, &[&str]); 3] = [
        (
            AMPLIFYUIBUILDER_COMPONENT,
            &["AppId", "CreatedAt", "EnvironmentName", "Id", "ModifiedAt"],
        ),
        (AMPLIFYUIBUILDER_FORM, &["AppId", "EnvironmentName", "Id"]),
        (
            AMPLIFYUIBUILDER_THEME,
            &["AppId", "CreatedAt", "EnvironmentName", "Id", "ModifiedAt"],
        ),
    ];

    for (resource_type, published) in rows {
        assert!(
            AWS_RESOURCE_TYPES.contains(&resource_type),
            "case {resource_type:?}: expected the type in the published catalogue, it is absent"
        );
        assert!(
            is_published(resource_type) && is_recognized(resource_type),
            "case {resource_type:?}: expected a type a template may declare, it is not"
        );
        // Amplify UI Builder mints the `id` every call on an entity takes, so the identifier in
        // force is the one that service answered rather than a name computed from the template.
        assert!(
            identity_is_service_assigned(resource_type),
            "case {resource_type:?}: expected the identity to be the one that service assigns, \
             it is not"
        );
        assert_eq!(
            published_attribute_names(resource_type),
            published,
            "case {resource_type:?}: expected the type's published return values {published:?}, \
             got {:?}",
            published_attribute_names(resource_type)
        );
        // Every call on one of these entities takes the app and the environment beside the id,
        // and no call moves an entity between them, so a declaration restating either asks for a
        // different entity.
        assert_eq!(
            replacement_properties(resource_type),
            &["AppId", "EnvironmentName"],
            "case {resource_type:?}: expected the app environment to be create-only, got {:?}",
            replacement_properties(resource_type)
        );
        // The entity is created under the `Name` the declaration states.
        assert!(
            name_property_candidates(resource_type).contains(&"Name".to_string()),
            "case {resource_type:?}: expected Name among the name properties, got {:?}",
            name_property_candidates(resource_type)
        );
    }

    // The catalogue is a list of names, not a shape test: a name it does not carry stays
    // unrecognised however close it is to one it does.
    for misspelled in [
        "AWS::AmplifyUIBuilder::Components",
        "AWS::AmplifyUiBuilder::Form",
        "AWS::AmplifyUIBuilder::CodegenJob",
    ] {
        assert!(
            !is_recognized(misspelled),
            "expected {misspelled:?} to stay unrecognised, it is recognised"
        );
    }
}
