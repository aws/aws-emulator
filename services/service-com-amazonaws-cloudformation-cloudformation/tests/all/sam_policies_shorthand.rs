// SPDX-License-Identifier: CC0-1.0

//! What a SAM declaration's `Policies` puts on the role the transform generates for it,
//! read back out of IAM over that service's own wire.
//!
//! The rule every row here holds the service to: **every `Policies` entry that is not an
//! AWS managed policy becomes exactly ONE inline policy on the role the transform
//! generates, named for the declaration and the entry's POSITION in the declared list
//! (`<LogicalId>RolePolicy<position>`), carrying the document the entry states — the
//! document itself where the entry is one, and the document the published policy template
//! grants over the resources its parameters name where the entry is the template shorthand
//! — while a string entry is ATTACHED as a managed policy and embeds nothing.** So the
//! deployed role grants exactly the actions the declaration asked for, and nothing it did
//! not.
//!
//! The recorded trace this suite was written for is ONE row of the table: a function
//! declaring several policy-template entries, whose generated role is then enumerated. The
//! rows differ along what the rule generalises over — the KIND of entry (a policy template,
//! an inline document, a managed policy ARN), how many of them and in which order, the
//! position a managed policy entry leaves behind, which published template is named, how
//! the parameter's value is stated (a literal name and a `Ref` to a resource in the same
//! template), the single-value and the list spelling of the property, and which serverless
//! resource the role is generated for (a function and a state machine).
//!
//! Every expected action and ARN is computed here from what the row declares (and, for a
//! generated name, from what the stack reports for that resource), never read back out of
//! the answer being checked.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::settle_the_store;
use crate::iam_identity_resources::xml_policy_document;
use crate::sam_transform::sam_template;
use crate::stack_resource_provisioning::stage_code_objects;
use crate::stack_lifecycle::{
    call, create_stack, iam, kv, member, router_with_engine, xml_text, xml_texts, ACCOUNT_ID,
    REGION,
};

/// The AWS managed policy every Lambda execution role the transform generates carries, as
/// the ARN IAM holds it under.
const BASIC_EXECUTION_ARN: &str = "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole";

/// The managed policy a function reading a DynamoDB stream needs on its execution role, as
/// the ARN IAM holds it under.
const DYNAMODB_STREAM_ARN: &str =
    "arn:aws:iam::aws:policy/service-role/AWSLambdaDynamoDBExecutionRole";

/// The managed policy a function polling an Amazon SQS queue needs on its execution role.
const SQS_QUEUE_ARN: &str = "arn:aws:iam::aws:policy/service-role/AWSLambdaSQSQueueExecutionRole";

/// An AWS-managed policy ARN a declaration can ask to have attached: IAM takes one as
/// present, so a string entry really reaches `AttachRolePolicy`.
const READ_ONLY_ARN: &str = "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess";

/// One inline policy the generated role must embed.
struct Inline {
    /// The policy's name, which the rule derives from the declaration and the position.
    name: String,
    /// Text the embedded document must carry: the actions the entry grants, and the ARNs
    /// they are granted on once CloudFormation has substituted them.
    granted: Vec<String>,
    /// Actions the entry does NOT grant, which the document must therefore not carry —
    /// the half of the rule that makes a probe of an ungranted action a denied one.
    withheld: Vec<String>,
}

impl Inline {
    fn new(name: &str, granted: &[&str], withheld: &[&str]) -> Self {
        Self {
            name: name.to_string(),
            granted: granted.iter().map(|text| (*text).to_string()).collect(),
            withheld: withheld.iter().map(|text| (*text).to_string()).collect(),
        }
    }
}

/// One row of the table.
struct Row {
    case: &'static str,
    stack: &'static str,
    template: String,
    /// The declaration whose generated role the row reads: the role's logical id is that
    /// id plus `Role`.
    owner: &'static str,
    /// Every inline policy the role must embed, in the order `ListRolePolicies` reports
    /// them.
    inline: Vec<Inline>,
    /// The managed policies the role must have attached, beside nothing else.
    attached: Vec<&'static str>,
}

/// The DynamoDB table `name` and its indexes, as the ARNs a table-scoped template's
/// statement is granted over once CloudFormation has substituted the partition, the Region
/// and the account.
fn table_arns(name: &str) -> Vec<String> {
    vec![
        format!("arn:aws:dynamodb:{REGION}:{ACCOUNT_ID}:table/{name}"),
        format!("arn:aws:dynamodb:{REGION}:{ACCOUNT_ID}:table/{name}/index/*"),
    ]
}

/// The bucket `name` and the objects in it, as the ARNs a bucket-scoped template's
/// statement is granted over. An Amazon S3 ARN carries neither a Region nor an account.
fn bucket_arns(name: &str) -> Vec<String> {
    vec![
        format!("arn:aws:s3:::{name}"),
        format!("arn:aws:s3:::{name}/*"),
    ]
}

/// A function declaration whose `Policies` is `policies`, spelled exactly as the row states
/// it (a list, or the single value the property also accepts).
fn function_with_policies(logical_id: &str, policies: &str) -> String {
    format!(
        r#""{logical_id}":{{"Type":"AWS::Serverless::Function","Properties":{{
             "Handler":"index.handler","Runtime":"python3.12",
             "CodeUri":"s3://code-bucket/{logical_id}.zip",
             "Policies":{policies}}}}}"#
    )
}

/// A function declaration whose `Events` map is `events`, with the `Policies` the row
/// states (an empty string for a declaration that states none).
fn function_with_events(logical_id: &str, policies: &str, events: &str) -> String {
    let policies = match policies.is_empty() {
        true => String::new(),
        false => format!(r#""Policies":{policies},"#),
    };
    format!(
        r#""{logical_id}":{{"Type":"AWS::Serverless::Function","Properties":{{
             "Handler":"index.handler","Runtime":"python3.12",
             "CodeUri":"s3://code-bucket/{logical_id}.zip",
             {policies}"Events":{events}}}}}"#
    )
}

/// One event-source-mapping event of `kind`, reading `source` under the member that kind
/// spells its source ARN with.
fn pull_event(name: &str, kind: &str, member: &str, source: &str) -> String {
    format!(
        r#""{name}":{{"Type":"{kind}","Properties":{{"{member}":"{source}",
             "StartingPosition":"LATEST"}}}}"#
    )
}

/// `granted` as the texts one inline policy's document must carry: the actions, and the
/// ARNs they are granted on.
fn carries(actions: &[&str], arns: &[String]) -> Vec<String> {
    let mut texts: Vec<String> = actions.iter().map(|text| (*text).to_string()).collect();
    texts.extend(arns.iter().cloned());
    texts
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one policy-template entry",
            stack: "sampolicy-one-template",
            template: sam_template(&format!(
                "{{{}}}",
                function_with_policies(
                    "Reader",
                    r#"[{"DynamoDBReadPolicy":{"TableName":"orders"}}]"#
                )
            )),
            owner: "Reader",
            inline: vec![Inline {
                name: "ReaderRolePolicy0".to_string(),
                granted: carries(
                    &["dynamodb:GetItem", "dynamodb:Query", "dynamodb:Scan"],
                    &table_arns("orders"),
                ),
                // A read template grants no write, which is what makes a write probe
                // against this role a denied one.
                withheld: vec![
                    "dynamodb:PutItem".to_string(),
                    "dynamodb:DeleteItem".to_string(),
                    "dynamodb:UpdateItem".to_string(),
                ],
            }],
            attached: vec![BASIC_EXECUTION_ARN],
        },
        Row {
            case: "two policy-template entries, named positionally in declaration order",
            stack: "sampolicy-two-templates",
            template: sam_template(&format!(
                "{{{}}}",
                function_with_policies(
                    "Probe",
                    r#"[{"DynamoDBCrudPolicy":{"TableName":"carts"}},
                        {"S3ReadPolicy":{"BucketName":"reports-bucket"}}]"#
                )
            )),
            owner: "Probe",
            inline: vec![
                Inline {
                    name: "ProbeRolePolicy0".to_string(),
                    granted: carries(
                        &[
                            "dynamodb:GetItem",
                            "dynamodb:PutItem",
                            "dynamodb:DeleteItem",
                            "dynamodb:BatchWriteItem",
                        ],
                        &table_arns("carts"),
                    ),
                    // An item-level template grants nothing on the table itself.
                    withheld: vec![
                        "dynamodb:DeleteTable".to_string(),
                        "dynamodb:CreateTable".to_string(),
                    ],
                },
                Inline {
                    name: "ProbeRolePolicy1".to_string(),
                    granted: carries(
                        &["s3:GetObject", "s3:ListBucket"],
                        &bucket_arns("reports-bucket"),
                    ),
                    withheld: vec!["s3:PutObject".to_string(), "s3:DeleteObject".to_string()],
                },
            ],
            attached: vec![BASIC_EXECUTION_ARN],
        },
        Row {
            case: "a managed policy ARN between a template and a document: the positions \
                   the entries hold are the ones they were declared at",
            stack: "sampolicy-mixed",
            template: sam_template(&format!(
                "{{{}}}",
                function_with_policies(
                    "Mixed",
                    &format!(
                        r#"["{READ_ONLY_ARN}",
                            {{"SQSSendMessagePolicy":{{"QueueName":"jobs"}}}},
                            {{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
                              "Action":"sns:Publish","Resource":"*"}}]}}]"#
                    )
                )
            )),
            owner: "Mixed",
            inline: vec![
                Inline::new(
                    "MixedRolePolicy1",
                    &[
                        "sqs:SendMessage*",
                        &format!("arn:aws:sqs:{REGION}:{ACCOUNT_ID}:jobs"),
                    ],
                    &["sqs:ReceiveMessage", "sqs:DeleteMessage"],
                ),
                Inline::new("MixedRolePolicy2", &["sns:Publish"], &["sns:Subscribe"]),
            ],
            // The string entry is attached, not embedded, beside the managed policy every
            // generated execution role carries.
            attached: vec![BASIC_EXECUTION_ARN, READ_ONLY_ARN],
        },
        Row {
            case: "the single-value spelling of the property: one template entry, not in a \
                   list",
            stack: "sampolicy-single-value",
            template: sam_template(&format!(
                "{{{}}}",
                function_with_policies(
                    "Lone",
                    r#"{"S3CrudPolicy":{"BucketName":"assets-bucket"}}"#
                )
            )),
            owner: "Lone",
            inline: vec![Inline {
                name: "LoneRolePolicy0".to_string(),
                granted: carries(
                    &["s3:GetObject", "s3:PutObject", "s3:DeleteObject"],
                    &bucket_arns("assets-bucket"),
                ),
                withheld: vec!["s3:PutBucketPolicy".to_string()],
            }],
            attached: vec![BASIC_EXECUTION_ARN],
        },
        Row {
            case: "a template with two parameters, and one whose statements are not \
                   resource-scoped",
            stack: "sampolicy-two-parameters",
            template: sam_template(&format!(
                "{{{}}}",
                function_with_policies(
                    "Streamer",
                    r#"[{"DynamoDBStreamReadPolicy":{"TableName":"events",
                          "StreamName":"2024-01-01T00:00:00.000"}},
                        {"CloudWatchPutMetricPolicy":{}}]"#
                )
            )),
            owner: "Streamer",
            inline: vec![
                Inline::new(
                    "StreamerRolePolicy0",
                    &[
                        "dynamodb:GetRecords",
                        "dynamodb:DescribeStream",
                        &format!(
                            "arn:aws:dynamodb:{REGION}:{ACCOUNT_ID}:table/events/stream/\
                             2024-01-01T00:00:00.000"
                        ),
                    ],
                    &["dynamodb:PutItem"],
                ),
                Inline::new(
                    "StreamerRolePolicy1",
                    &["cloudwatch:PutMetricData"],
                    &["cloudwatch:GetMetricData"],
                ),
            ],
            attached: vec![BASIC_EXECUTION_ARN],
        },
        Row {
            case: "a function reading a DynamoDB stream: its role carries the managed \
                   policy that source needs beside the entries it declares",
            stack: "sampolicy-stream-event",
            template: sam_template(&format!(
                "{{{}}}",
                function_with_events(
                    "Ledger",
                    r#"[{"DynamoDBReadPolicy":{"TableName":"ledger"}}]"#,
                    &format!(
                        "{{{}}}",
                        pull_event(
                            "Records",
                            "DynamoDB",
                            "Stream",
                            &format!(
                                "arn:aws:dynamodb:{REGION}:{ACCOUNT_ID}:table/ledger/stream/\
                                 2024-01-01T00:00:00.000"
                            )
                        )
                    )
                )
            )),
            owner: "Ledger",
            inline: vec![Inline {
                name: "LedgerRolePolicy0".to_string(),
                granted: carries(&["dynamodb:GetItem"], &table_arns("ledger")),
                withheld: vec!["dynamodb:PutItem".to_string()],
            }],
            attached: vec![BASIC_EXECUTION_ARN, DYNAMODB_STREAM_ARN],
        },
        Row {
            case: "two event sources needing a policy, one of them declared twice: each \
                   policy attached once, and no Policies entry at all",
            stack: "sampolicy-two-event-sources",
            template: sam_template(&format!(
                "{{{}}}",
                function_with_events(
                    "Pump",
                    "",
                    &format!(
                        "{{{},{},{}}}",
                        pull_event(
                            "Jobs",
                            "SQS",
                            "Queue",
                            &format!("arn:aws:sqs:{REGION}:{ACCOUNT_ID}:jobs")
                        ),
                        pull_event(
                            "Retries",
                            "SQS",
                            "Queue",
                            &format!("arn:aws:sqs:{REGION}:{ACCOUNT_ID}:retries")
                        ),
                        pull_event(
                            "Records",
                            "DynamoDB",
                            "Stream",
                            &format!(
                                "arn:aws:dynamodb:{REGION}:{ACCOUNT_ID}:table/audit/stream/\
                                 2024-01-01T00:00:00.000"
                            )
                        )
                    )
                )
            )),
            owner: "Pump",
            inline: vec![],
            attached: vec![BASIC_EXECUTION_ARN, DYNAMODB_STREAM_ARN, SQS_QUEUE_ARN],
        },
        Row {
            case: "a state machine's policies, on the role generated for it",
            stack: "sampolicy-state-machine",
            template: sam_template(
                r#"{"Flow":{"Type":"AWS::Serverless::StateMachine","Properties":{
                     "Definition":{"StartAt":"Done","States":{"Done":{"Type":"Succeed"}}},
                     "Policies":[{"LambdaInvokePolicy":{"FunctionName":"worker"}}]}}}"#,
            ),
            owner: "Flow",
            inline: vec![Inline::new(
                "FlowRolePolicy0",
                &[
                    "lambda:InvokeFunction",
                    &format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:worker*"),
                ],
                &["lambda:UpdateFunctionCode"],
            )],
            // A state machine's role carries no managed policy of the transform's own: only
            // what the declaration names, and this one names none.
            attached: vec![],
        },
    ]
}

/// Deploy `template` as stack `stack`, settle the store and assert the stack completed.
async fn deploy(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    stack: &str,
    template: &str,
) {
    stage_code_objects(router, case, template).await;
    let (code, body) = create_stack(
        router,
        stack,
        template,
        &kv(&[
            ("Capabilities.member.1", "CAPABILITY_IAM"),
            ("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
        ]),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateStack to be accepted, got {code} {body}"
    );
    settle_the_store(router, engine).await;
    let (code, body) = call(router, "DescribeStacks", &[member("StackName", stack)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStacks to answer, got {code} {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to deploy the role the transform generated, got \
         {body}"
    );
}

/// The physical id the stack reports for one logical id — for a generated role, the name
/// IAM holds it under.
async fn physical_id(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> String {
    let (code, body) = call(
        router,
        "DescribeStackResources",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStackResources({logical_id}) to answer, got {code} \
         {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_else(|| {
        panic!("case {case:?}: expected a physical id for {logical_id}, got {body}")
    })
}

/// The inline policy names IAM reports for `role`.
///
/// `ListRolePolicies` answers them in order, and a positional name sorts the way it was
/// declared, so this is the declaration order the rule states.
async fn inline_policy_names(router: &MultiServiceRouter, case: &str, role: &str) -> Vec<String> {
    let (code, body) = iam(router, "ListRolePolicies", &[("RoleName", role)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected ListRolePolicies({role:?}) to answer, got {code} {body}"
    );
    xml_texts(&body, "member")
}

/// The document IAM holds under one inline policy name.
async fn inline_document(
    router: &MultiServiceRouter,
    case: &str,
    role: &str,
    policy: &str,
) -> String {
    let (code, body) = iam(
        router,
        "GetRolePolicy",
        &[("RoleName", role), ("PolicyName", policy)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected GetRolePolicy({role:?}, {policy:?}) to answer, got {code} \
         {body}"
    );
    xml_policy_document(&body, "PolicyDocument").unwrap_or_else(|| {
        panic!("case {case:?}: expected a document for {policy:?}, got {body}")
    })
}

/// The managed policies IAM reports as attached to `role`.
async fn attached_policies(router: &MultiServiceRouter, case: &str, role: &str) -> Vec<String> {
    let (code, body) = iam(router, "ListAttachedRolePolicies", &[("RoleName", role)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected ListAttachedRolePolicies({role:?}) to answer, got {code} \
         {body}"
    );
    let mut arns = xml_texts(&body, "PolicyArn");
    arns.sort();
    arns
}

#[tokio::test]
async fn every_policies_entry_becomes_one_positionally_named_inline_policy() {
    let (router, engine) = router_with_engine();
    for row in rows() {
        let case = row.case;
        deploy(&router, &engine, case, row.stack, &row.template).await;
        let role = physical_id(
            &router,
            case,
            row.stack,
            &format!("{}Role", row.owner),
        )
        .await;

        let expected: Vec<String> = row.inline.iter().map(|inline| inline.name.clone()).collect();
        let names = inline_policy_names(&router, case, &role).await;
        assert_eq!(
            names, expected,
            "case {case:?}: expected one inline policy per entry, named positionally and in \
             declaration order ({expected:?}), got {names:?}"
        );

        for inline in &row.inline {
            let document = inline_document(&router, case, &role, &inline.name).await;
            for granted in &inline.granted {
                assert!(
                    document.contains(granted.as_str()),
                    "case {case:?} policy {:?}: expected the document to grant {granted:?}, \
                     got {document:?}",
                    inline.name
                );
            }
            for withheld in &inline.withheld {
                assert!(
                    !document.contains(withheld.as_str()),
                    "case {case:?} policy {:?}: expected the document NOT to grant \
                     {withheld:?}, got {document:?}",
                    inline.name
                );
            }
        }

        let mut expected: Vec<String> = row
            .attached
            .iter()
            .map(|arn| (*arn).to_string())
            .collect();
        expected.sort();
        let attached = attached_policies(&router, case, &role).await;
        assert_eq!(
            attached, expected,
            "case {case:?}: expected exactly the managed policies the declaration asks for \
             ({expected:?}), got {attached:?}"
        );
    }
}

/// RULE (the parameter's value half): a template parameter carries the value the
/// declaration states for it, so a `Ref` to a resource in the same template grants the
/// policy over the resource CloudFormation really created — the bucket's own name, which
/// nothing in the template spells.
#[tokio::test]
async fn a_policy_templates_parameter_resolves_a_ref_to_the_resource_it_names() {
    let case = "a template parameter stated as a Ref";
    let (router, engine) = router_with_engine();
    let template = sam_template(&format!(
        r#"{{"Reports":{{"Type":"AWS::S3::Bucket"}},{}}}"#,
        function_with_policies(
            "Publisher",
            r#"[{"S3WritePolicy":{"BucketName":{"Ref":"Reports"}}}]"#
        )
    ));
    deploy(&router, &engine, case, "sampolicy-ref", &template).await;

    // The bucket's name is the physical id CloudFormation generated for it: the expected
    // ARNs are built from what the stack reports, not from anything the template said.
    let bucket = physical_id(&router, case, "sampolicy-ref", "Reports").await;
    let role = physical_id(&router, case, "sampolicy-ref", "PublisherRole").await;

    let names = inline_policy_names(&router, case, &role).await;
    assert_eq!(
        names,
        vec!["PublisherRolePolicy0".to_string()],
        "case {case:?}: expected the one entry embedded at its position, got {names:?}"
    );
    let document = inline_document(&router, case, &role, "PublisherRolePolicy0").await;
    for granted in carries(&["s3:PutObject", "s3:DeleteObject"], &bucket_arns(&bucket)) {
        assert!(
            document.contains(granted.as_str()),
            "case {case:?}: expected the document to grant {granted:?} on the bucket the \
             stack created, got {document:?}"
        );
    }
    assert!(
        !document.contains("s3:GetObject"),
        "case {case:?}: a write template grants no read, got {document:?}"
    );
}

/// RULE (the empty half): a declaration that states no `Policies` at all generates a role
/// with no inline policy — the permissions a template did not ask for are not invented.
#[tokio::test]
async fn a_function_declaring_no_policies_generates_a_role_with_none() {
    let case = "no Policies at all";
    let (router, engine) = router_with_engine();
    let template = sam_template(
        r#"{"Bare":{"Type":"AWS::Serverless::Function","Properties":{
             "Handler":"index.handler","Runtime":"python3.12",
             "CodeUri":"s3://code-bucket/bare.zip"}}}"#,
    );
    deploy(&router, &engine, case, "sampolicy-none", &template).await;
    let role = physical_id(&router, case, "sampolicy-none", "BareRole").await;
    let names = inline_policy_names(&router, case, &role).await;
    assert!(
        names.is_empty(),
        "case {case:?}: expected no inline policy on a role whose declaration asks for \
         none, got {names:?}"
    );
    assert_eq!(
        attached_policies(&router, case, &role).await,
        vec![BASIC_EXECUTION_ARN.to_string()],
        "case {case:?}: expected the generated execution role to carry the basic execution \
         managed policy alone"
    );
}
