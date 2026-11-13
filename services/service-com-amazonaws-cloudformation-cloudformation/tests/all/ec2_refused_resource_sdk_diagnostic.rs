// SPDX-License-Identifier: CC0-1.0

//! What a resource AMAZON EC2 refused reports on its `CREATE_FAILED` event, end to end over
//! the awsQuery wire.
//!
//! RULE under test: **a resource provider reports a refused call as the exception its SDK
//! raised — the owning service's own sentence followed by that SDK's diagnostic of the call —
//! so the status and the error CODE the owning service answered are readable out of the
//! resource's `ResourceStatusReason`. Amazon EC2's SDK words that diagnostic as
//! `(Service: Ec2; Status Code: <status>; Error Code: <code>; Request ID: <id>; Proxy: null)`,
//! whatever type of that service failed and whichever of its calls was refused.**
//!
//! Why it matters: a client classifies a failed deployment by parsing the relayed service
//! error — that is how an already-in-use name is told apart from a quota, a permission or a
//! malformed-value refusal, and what deploy tooling and runbooks match on. A reason carrying
//! only free text leaves no error code and no status to branch on, so every launch-template
//! failure reads the same.
//!
//! The rows differ along what the rule generalises over: WHICH type of that service failed (a
//! launch template, a subnet, a security group, a VPC), which call of it was refused, and the
//! error CODE it answered — a name already in use, a VPC it does not hold, a value it cannot
//! read and a block size it does not permit — together with the `HandlerErrorCode` class each
//! of those codes falls in. The `Proxy` field is the same for every row because every one of
//! these calls is made directly, with no proxy between.

use std::time::Duration;

use crate::stack_lifecycle::{create_stack, ec2, resource_event_reason, router};

/// The SDK's own name for Amazon EC2, as the exception its SDK raises states it.
const EC2_SERVICE: &str = "Ec2";

/// The HTTP status Amazon EC2 answers a refused request with: `ec2Query` carries every
/// client-side refusal at 400.
const REFUSED_STATUS: u16 = 400;

/// What the diagnostic states for a call made with no proxy configured, which every call this
/// emulator makes in process is.
const DIRECT_CALL_PROXY: &str = "Proxy: null";

/// How long a row waits for the failed resource's event to be there before giving up.
///
/// Generous on purpose: the only thing being waited FOR is the deployment recording the
/// resource's failure, which is on the stack's own clock, and the wait ends as soon as the
/// event is readable rather than after a fixed time.
const EVENT_DEADLINE: Duration = Duration::from_secs(60);

/// How long one look-again waits before asking the event stream once more.
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// The AMI a launch template's data names. The id form is Amazon EC2's own (`ami-` plus 17
/// hexadecimal characters), which is what that service reads an image id as.
const IMAGE_ID: &str = "ami-00000000000000001";

/// A VPC id of the form Amazon EC2 mints (`vpc-` plus 17 hexadecimal characters) that no VPC
/// of the account answers to, so a declaration naming it names a VPC that service does not
/// hold.
const ABSENT_VPC_ID: &str = "vpc-0123456789abcdef0";

/// The `HandlerErrorCode` classes the rows' codes fall in — all values of the model's own
/// `HandlerErrorCode` enum.
const ALREADY_EXISTS: &str = "AlreadyExists";
const NOT_FOUND: &str = "NotFound";
const INVALID_REQUEST: &str = "InvalidRequest";

/// How much of the owning service's own sentence a row holds the quoted message to.
enum Sentence {
    /// The WHOLE sentence, for a refusal whose text is fixed and carries no variable part.
    Whole(&'static str),
    /// A fragment of it, for a refusal whose text names a value taken from the request.
    Carrying(&'static str),
}

/// One row: the stack to deploy, the resource Amazon EC2 refuses, and what its reason must
/// report.
struct Row {
    case: &'static str,
    stack: &'static str,
    template: &'static str,
    logical_id: &'static str,
    /// A launch template name to register in Amazon EC2 BEFORE the deployment, for a row whose
    /// refusal is that the name is already taken.
    registered_launch_template: Option<&'static str>,
    /// The error code that service answered, which the diagnostic must state.
    code: &'static str,
    /// The `HandlerErrorCode` class that code falls in.
    class: &'static str,
    /// What the quoted message must carry of the owning service's own sentence.
    sentence: Sentence,
}

/// The name both the pre-registered launch template and the declared one carry.
const TAKEN_TEMPLATE_NAME: &str = "ec2-diag-taken-template";

/// A launch template declared under a name Amazon EC2 already holds a template under: that
/// service arbitrates its own namespace and refuses the create.
const TAKEN_NAME_TEMPLATE: &str = r#"{"Resources":{
    "Lt":{"Type":"AWS::EC2::LaunchTemplate","Properties":{
      "LaunchTemplateName":"ec2-diag-taken-template",
      "LaunchTemplateData":{"ImageId":"ami-00000000000000001","InstanceType":"t3.micro"}}}}}"#;

/// A subnet in a VPC Amazon EC2 holds nothing for: a different type and a different call of
/// the same service, refused because the declaration names a resource that is not there.
const ABSENT_VPC_SUBNET_TEMPLATE: &str = r#"{"Resources":{
    "Subnet":{"Type":"AWS::EC2::Subnet","Properties":{
      "VpcId":"vpc-0123456789abcdef0","CidrBlock":"10.0.1.0/24"}}}}"#;

/// A security group in that same absent VPC: another type, another call, and the same error
/// code — the diagnostic reports the code whatever call answered it.
const ABSENT_VPC_GROUP_TEMPLATE: &str = r#"{"Resources":{
    "Group":{"Type":"AWS::EC2::SecurityGroup","Properties":{
      "GroupDescription":"a group in a VPC the service does not hold",
      "VpcId":"vpc-0123456789abcdef0"}}}}"#;

/// A VPC whose declared block is no CIDR block at all, which that service refuses on the
/// VALUE.
const UNREADABLE_CIDR_TEMPLATE: &str = r#"{"Resources":{
    "Vpc":{"Type":"AWS::EC2::VPC","Properties":{"CidrBlock":"not-a-cidr-block"}}}}"#;

/// A VPC whose declared block is a CIDR block of a size that service does not permit, which it
/// refuses under a code of its own — a different refusal of the same member.
const OVERSIZE_CIDR_TEMPLATE: &str = r#"{"Resources":{
    "Vpc":{"Type":"AWS::EC2::VPC","Properties":{"CidrBlock":"10.0.0.0/8"}}}}"#;

const ROWS: &[Row] = &[
    Row {
        case: "a launch template under a name the service already holds",
        stack: "ec2-diag-taken-name",
        template: TAKEN_NAME_TEMPLATE,
        logical_id: "Lt",
        registered_launch_template: Some(TAKEN_TEMPLATE_NAME),
        code: "InvalidLaunchTemplateName.AlreadyExistsException",
        class: ALREADY_EXISTS,
        // That service's sentence for this refusal is FIXED: a create names ONE launch
        // template name, so the value the caller must change is the one it just sent and the
        // service does not repeat it back.
        sentence: Sentence::Whole("Launch template name already in use."),
    },
    Row {
        case: "a subnet in a VPC the service does not hold",
        stack: "ec2-diag-absent-vpc-subnet",
        template: ABSENT_VPC_SUBNET_TEMPLATE,
        logical_id: "Subnet",
        registered_launch_template: None,
        code: "InvalidVpcID.NotFound",
        class: NOT_FOUND,
        sentence: Sentence::Carrying(ABSENT_VPC_ID),
    },
    Row {
        case: "a security group in a VPC the service does not hold",
        stack: "ec2-diag-absent-vpc-group",
        template: ABSENT_VPC_GROUP_TEMPLATE,
        logical_id: "Group",
        registered_launch_template: None,
        code: "InvalidVpcID.NotFound",
        class: NOT_FOUND,
        sentence: Sentence::Carrying(ABSENT_VPC_ID),
    },
    Row {
        case: "a VPC whose declared block is no CIDR block at all",
        stack: "ec2-diag-unreadable-cidr",
        template: UNREADABLE_CIDR_TEMPLATE,
        logical_id: "Vpc",
        registered_launch_template: None,
        code: "InvalidParameterValue",
        class: INVALID_REQUEST,
        sentence: Sentence::Carrying("not-a-cidr-block"),
    },
    Row {
        case: "a VPC whose declared block is of a size the service does not permit",
        stack: "ec2-diag-oversize-cidr",
        template: OVERSIZE_CIDR_TEMPLATE,
        logical_id: "Vpc",
        registered_launch_template: None,
        code: "InvalidVpc.Range",
        class: INVALID_REQUEST,
        sentence: Sentence::Carrying("10.0.0.0/8"),
    },
];

/// The quoted provider message and the reported `HandlerErrorCode` of one
/// `ResourceStatusReason`.
fn quoted_and_class(case: &str, reason: &str) -> (String, String) {
    const OPENS: &str = "Resource handler returned message: \"";
    const TOKEN: &str = "\" (RequestToken: ";
    const CLASS: &str = "HandlerErrorCode: ";
    let rest = reason.strip_prefix(OPENS).unwrap_or_else(|| {
        panic!("case {case:?}: expected the reason to open with {OPENS:?}, got {reason:?}")
    });
    let (quoted, tail) = rest.split_once(TOKEN).unwrap_or_else(|| {
        panic!("case {case:?}: expected a quoted message and a RequestToken, got {reason:?}")
    });
    let class = tail
        .split_once(CLASS)
        .and_then(|(_, class)| class.strip_suffix(')'))
        .unwrap_or_else(|| panic!("case {case:?}: expected a HandlerErrorCode, got {reason:?}"));
    (quoted.to_string(), class.to_string())
}

/// The owning service's own sentence and the diagnostic its SDK appended, split apart.
fn sentence_and_diagnostic(case: &str, quoted: &str) -> (String, String) {
    const OPENS: &str = " (Service: ";
    let (sentence, diagnostic) = quoted.rsplit_once(OPENS).unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected the SDK diagnostic after the service's own sentence, got \
             {quoted:?}"
        )
    });
    let envelope = diagnostic.strip_suffix(')').unwrap_or_else(|| {
        panic!("case {case:?}: expected the diagnostic to close, got {quoted:?}")
    });
    (sentence.to_string(), envelope.to_string())
}

/// Hold one diagnostic to the shape Amazon EC2's SDK words: the service, the status, the error
/// code, the id of the call and the proxy it was made through — and nothing else. Answers the
/// request id it stated, so a caller can hold THAT to its own form.
fn assert_diagnostic(case: &str, envelope: &str, code: &str) -> String {
    let fields: Vec<&str> = envelope.split("; ").collect();
    assert_eq!(
        fields.len(),
        5,
        "case {case:?}: expected the service, the status, the error code, the call's id and the \
         proxy and nothing else, got {envelope:?}"
    );
    assert_eq!(
        fields[0], EC2_SERVICE,
        "case {case:?}: expected the SDK's own name for the service, got {:?}",
        fields[0]
    );
    assert_eq!(
        fields[1],
        format!("Status Code: {REFUSED_STATUS}"),
        "case {case:?}: expected the status the owning service answered, got {:?}",
        fields[1]
    );
    assert_eq!(
        fields[2],
        format!("Error Code: {code}"),
        "case {case:?}: expected the error code the owning service answered, got {:?}",
        fields[2]
    );
    let request_id = fields[3]
        .strip_prefix("Request ID: ")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Request ID, got {:?}", fields[3]));
    assert!(
        is_a_request_id(request_id),
        "case {case:?}: expected the id of the call in the form that service's answers carry, \
         got {request_id:?}"
    );
    assert_eq!(
        fields[4], DIRECT_CALL_PROXY,
        "case {case:?}: expected the proxy of a direct call, got {:?}",
        fields[4]
    );
    request_id.to_string()
}

/// Whether `value` has the 8-4-4-4-12 lowercase-hexadecimal form an AWS request id carries.
fn is_a_request_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups.iter().all(|group| {
            group
                .chars()
                .all(|held| held.is_ascii_hexdigit() && !held.is_ascii_uppercase())
        })
}

/// Register one launch template in Amazon EC2 directly, so a stack that declares the same name
/// meets a name that service really holds.
async fn register_launch_template(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    name: &str,
) {
    let (status, body) = ec2(
        router,
        "CreateLaunchTemplate",
        &[
            ("LaunchTemplateName", name),
            ("LaunchTemplateData.ImageId", IMAGE_ID),
            ("LaunchTemplateData.InstanceType", "t3.micro"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateLaunchTemplate({name}) must answer 200, got {status} {body}"
    );
}

/// The `ResourceStatusReason` of `logical_id`'s `CREATE_FAILED` event, looked for again until
/// the deployment has recorded it or [`EVENT_DEADLINE`] passes.
async fn failure_reason(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> String {
    let deadline = tokio::time::Instant::now() + EVENT_DEADLINE;
    loop {
        let reason = resource_event_reason(router, stack, logical_id, "CREATE_FAILED").await;
        if !reason.is_empty() {
            return reason;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: {logical_id} never recorded a CREATE_FAILED reason"
        );
        tokio::time::sleep(EVENT_POLL_INTERVAL).await;
    }
}

/// Deploy one row's stack and answer the quoted message and the class its failed resource
/// reports.
async fn refusal_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    row: &Row,
    stack: &str,
) -> (String, String) {
    let (status_code, created) = create_stack(router, stack, row.template, &[]).await;
    assert_eq!(
        status_code, 200,
        "case {:?}: CreateStack must answer 200, got {status_code} {created}",
        row.case
    );
    let reason = failure_reason(router, row.case, stack, row.logical_id).await;
    quoted_and_class(row.case, &reason)
}

#[tokio::test]
async fn a_resource_ec2_refused_carries_that_service_s_status_and_error_code() {
    for row in ROWS {
        let router = router();
        if let Some(name) = row.registered_launch_template {
            register_launch_template(&router, row.case, name).await;
        }
        let (quoted, class) = refusal_of(&router, row, row.stack).await;
        let (sentence, envelope) = sentence_and_diagnostic(row.case, &quoted);
        match row.sentence {
            Sentence::Whole(whole) => assert_eq!(
                sentence, whole,
                "case {:?}: expected the owning service's own fixed sentence, got {sentence:?}",
                row.case
            ),
            Sentence::Carrying(fragment) => assert!(
                sentence.contains(fragment),
                "case {:?}: expected the owning service's own sentence carrying {fragment:?}, \
                 got {sentence:?}",
                row.case
            ),
        }
        assert_diagnostic(row.case, &envelope, row.code);
        assert_eq!(
            class, row.class,
            "case {:?}: expected the {} class, got {class:?}",
            row.case, row.class
        );
    }
}

#[tokio::test]
async fn the_diagnostic_states_each_refused_call_s_own_request_id() {
    // The id in the diagnostic identifies the CALL, so two deployments refused the same way
    // state two different ids: an id frozen into the wording would identify nothing.
    let case = "two stacks declaring the same taken launch template name";
    let row = &ROWS[0];
    let router = router();
    register_launch_template(&router, case, TAKEN_TEMPLATE_NAME).await;

    let mut ids = Vec::new();
    for stack in ["ec2-diag-ids-first", "ec2-diag-ids-second"] {
        let (quoted, _) = refusal_of(&router, row, stack).await;
        let (_, envelope) = sentence_and_diagnostic(case, &quoted);
        ids.push(assert_diagnostic(case, &envelope, row.code));
    }
    assert_ne!(
        ids[0], ids[1],
        "case {case:?}: expected each refused call's own Request ID, got {ids:?}"
    );
}
