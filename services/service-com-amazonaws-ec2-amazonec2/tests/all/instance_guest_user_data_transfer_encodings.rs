// SPDX-License-Identifier: CC0-1.0

//! The TRANSFER ENCODINGS a launch's user data may arrive wrapped in, driven over the wire the
//! service actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! RULE: "the user data to make available to the instance" (`RunInstancesRequest$UserData`) is
//! a program the guest's init system RUNS, and what it runs is the DOCUMENT inside the
//! encodings the payload carries: cloud-init decompresses user data that arrived gzipped and
//! decodes user data that arrived base64 before it reads the form, so a payload wrapped in
//! either — or in both, or in one of them twice, which is what a client that encodes a
//! document itself and hands it to an SDK that encodes it again produces — gives the guest the
//! document inside it. A payload that carries no document the emulator reads a form for gives
//! the guest nothing, which is what keeps this from inventing a program for arbitrary bytes.
//!
//! ONE program is carried by every row — it writes a marker of its own to the guest's console
//! device — so the rows differ ONLY in how the payload that carries it is wrapped, and the
//! console says whether the guest ran it. The negative row carries the same program under an
//! encoding that leaves no document behind, so a row passing on output nothing produced is not
//! possible.
//!
//! The wait is on the CONDITION the console is read for — the boot's own program having run —
//! looked for again until it holds, never on a span of the clock.

use std::time::{Duration, Instant};

use base64::Engine as _;
use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, ImageSpec};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, elements, member};

/// The Region every call here is signed for; it only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// The instance type every row launches.
const INSTANCE_TYPE: &str = "t3.micro";

/// How long a row keeps looking for the boot's own output, and how long it leaves between
/// looks. Generous on purpose: the wait is on the CONDITION, so a fast machine spends none of
/// it.
const DEADLINE: Duration = Duration::from_secs(60);
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(50);

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One ec2Query call through the shared harness, which has to answer 200.
async fn api(router: &MultiServiceRouter, case: &str, action: &str, params: &[(String, String)]) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {action} must answer 200, got {status}: {body}"
    );
    body
}

/// One emulator over its own engine, with one AMI of the calling account seeded the way a
/// register operation would store it — answered beside its image id.
fn emulator() -> (MultiServiceRouter, String) {
    let (router, engine) = super::ec2_query::emulator();
    let image_id = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        guard.insert_image(ImageSpec {
            name: "instance-guest-user-data-transfer-encodings".to_string(),
            ..ImageSpec::default()
        })
    };
    (router, image_id)
}

/// The script every row carries: it writes its own marker to the guest's console device, so
/// the console says whether the guest ran it.
fn marking_program(tag: &str) -> String {
    format!("#!/bin/sh\necho \"{tag} ran\" > /dev/console\n")
}

/// `text` as a gzip member, which is what user data that arrived COMPRESSED carries.
fn gzipped(text: &str) -> Vec<u8> {
    use std::io::Write as _;

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder
        .write_all(text.as_bytes())
        .expect("the program compresses");
    encoder.finish().expect("the gzip member closes")
}

/// `bytes` as the base64 text that stands for them.
fn encoded(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// One row: how the payload carrying the program is wrapped, and whether the guest runs it.
struct Row {
    case: &'static str,
    /// The marker the row's own program writes.
    tag: &'static str,
    /// The payload the API's `UserData` stands for — what the instance is given once the
    /// service has decoded the member itself.
    payload: Vec<u8>,
    /// Whether the guest runs the program the payload carries.
    runs: bool,
}

/// The rows the rule is asserted over: the same program under each wrapping.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the document itself, as the API's own encoding carries it",
            tag: "PLAIN",
            payload: marking_program("PLAIN").into_bytes(),
            runs: true,
        },
        Row {
            case: "a document the client encoded before the SDK encoded it",
            tag: "ENCODED-ONCE",
            payload: encoded(marking_program("ENCODED-ONCE").as_bytes()).into_bytes(),
            runs: true,
        },
        Row {
            case: "a document encoded twice over before that",
            tag: "ENCODED-TWICE",
            payload: encoded(encoded(marking_program("ENCODED-TWICE").as_bytes()).as_bytes())
                .into_bytes(),
            runs: true,
        },
        Row {
            case: "a gzipped document",
            tag: "GZIPPED",
            payload: gzipped(&marking_program("GZIPPED")),
            runs: true,
        },
        Row {
            case: "a gzipped document the client then encoded",
            tag: "GZIPPED-ENCODED",
            payload: encoded(&gzipped(&marking_program("GZIPPED-ENCODED"))).into_bytes(),
            runs: true,
        },
        Row {
            // The same program, wrapped in an encoding that leaves bytes of no form behind:
            // the document inside carries no header this emulator reads a program for.
            case: "a payload that carries no document a form is read for",
            tag: "UNREAD",
            payload: format!("#include\n{}", marking_program("UNREAD")).into_bytes(),
            runs: false,
        },
    ]
}

/// Launch ONE instance whose `UserData` stands for `payload`, answering its instance id.
async fn launch(
    router: &MultiServiceRouter,
    case: &str,
    image_id: &str,
    payload: &[u8],
) -> String {
    let body = api(
        router,
        case,
        "RunInstances",
        &[
            p("ImageId", image_id),
            p("InstanceType", INSTANCE_TYPE),
            p("MinCount", "1"),
            p("MaxCount", "1"),
            p("UserData", encoded(payload)),
        ],
    )
    .await;
    elements(&body, "instanceId")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances must report an instance: {body}"))
}

/// One instance's console as text — empty for an instance whose guest has written nothing,
/// which is an instance that carries no `output` member at all.
async fn console(router: &MultiServiceRouter, case: &str, instance_id: &str) -> String {
    let body = api(
        router,
        case,
        "GetConsoleOutput",
        &[p("InstanceId", instance_id), p("Latest", "true")],
    )
    .await;
    let Some(text) = member(&body, "output") else {
        return String::new();
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&text)
        .unwrap_or_else(|error| panic!("case {case:?}: the output must be base64 ({error})"));
    String::from_utf8(bytes)
        .unwrap_or_else(|error| panic!("case {case:?}: the console text must be UTF-8: {error}"))
}

/// Whether the guest of `instance_id` has booted at all yet, as its own console says.
fn has_booted(console: &str) -> bool {
    console.contains("ci-info: boot-id: ")
}

/// RULE: the program a guest runs is the document inside the encodings its user data carried.
#[tokio::test]
async fn a_guest_runs_the_document_inside_the_encodings_its_user_data_carried() {
    let (router, image_id) = emulator();
    let rows = rows();
    // Every row's instance is launched before anything is read, so the whole table shares one
    // launch span rather than waiting one out per row.
    let mut instances: Vec<String> = Vec::new();
    for row in &rows {
        instances.push(launch(&router, row.case, &image_id, &row.payload).await);
    }

    for (row, instance_id) in rows.iter().zip(&instances) {
        let case = row.case;
        let marker = format!("{} ran", row.tag);
        // The boot is what runs the program, so the wait is for the guest to have booted —
        // and then the console either carries the row's marker or it does not.
        let giving_up_at = Instant::now() + DEADLINE;
        let mut text = console(&router, case, instance_id).await;
        while !(has_booted(&text) && (!row.runs || text.contains(&marker)))
            && Instant::now() < giving_up_at
        {
            tokio::time::sleep(LOOK_AGAIN_AFTER).await;
            text = console(&router, case, instance_id).await;
        }
        assert!(
            has_booted(&text),
            "case {case:?}: the guest must come up, got {text:?}"
        );
        assert_eq!(
            text.contains(&marker),
            row.runs,
            "case {case:?}: expected the guest to {} the program the payload carries, got \
             {text:?}",
            if row.runs { "run" } else { "run none of" }
        );
    }
}
