// SPDX-License-Identifier: CC0-1.0

//! The SLOW LOG a cache's engine keeps, as a client of the published endpoint reads it.
//!
//! The rule under test: a cache records in its slow log every command whose own execution
//! reaches the `slowlog-log-slower-than` microseconds its cache parameter group states, keeps
//! at most `slowlog-max-len` of them with the most recent first, and answers them to
//! `SLOWLOG GET` — `SLOWLOG LEN` counting what it holds and `SLOWLOG RESET` discarding them.
//! The duration compared against the threshold is the one the cache really took, so a
//! threshold no command reaches leaves the log empty.
//!
//! The rows span what the rule generalises over: the THRESHOLD the parameter group states
//! (the published bottom of that parameter's range, which records every command, and a value
//! no command reaches), the DEPTH it keeps, which command was executed, and the `SLOWLOG`
//! subcommand the client reads with. Where the records a cache DELIVERS to the CloudWatch
//! Logs group its `LogDeliveryConfiguration` names are asserted, the suite is
//! `core/router/tests/all/elasticache_slow_log_delivery_to_cloudwatch_logs.rs`: a log group
//! belongs to another service, so what landed in one is only readable there.
//!
//! The two string commands the rows read the recorded entries through — `GETRANGE` and
//! `SETRANGE` — are asserted here as well, over a table of the offsets the engine resolves.
//!
//! Every assertion goes over a real socket the core opened for the cache, dialled at the
//! address the control plane published; nothing reaches into the service's state.

use crate::cache_data_plane::{command, endpoint_of, open, Resp, Session};
use crate::harness::{params, poll_until, Emulator, REGION};

/// The parameter-group family of the version line every cluster below runs — one of
/// `CacheParameterGroupFamily`'s own valid values.
const FAMILY: &str = "redis7";

/// The bottom of the `slowlog-log-slower-than` range the published catalog states for that
/// family (`0-`), which records every command: "a value of zero forces the logging of every
/// command".
///
/// `pub(crate)` for the reason the helpers below are: a second suite that needs a cache whose
/// engine records every command states the same bottom of the same range.
pub(crate) const EVERY_COMMAND: &str = "0";

/// A threshold no command of this emulator reaches: a minute, in microseconds. Nothing waits
/// for it — it is the row showing a command that was not slow is not recorded.
const NO_COMMAND: &str = "60000000";

/// Create a cache parameter group holding the slow-log parameters a case states, and answer
/// its name.
///
/// `pub(crate)` for the reason the rest of this suite's helpers are: a second suite that runs
/// a cache under a stated slow-log policy states it the same way, and a second copy of this
/// setup would be a second thing to keep correct.
pub(crate) async fn recording_parameter_group(
    emulator: &Emulator,
    case: &str,
    threshold: &str,
    depth: &str,
) -> String {
    let group = format!("{case}-parameters");
    let created = emulator
        .call(
            "CreateCacheParameterGroup",
            &params(&[
                ("CacheParameterGroupName", &group),
                ("CacheParameterGroupFamily", FAMILY),
                ("Description", "the slow-log parameters under test"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case}: the parameter group must be created, got {} {}",
        created.status, created.body
    );
    let modified = emulator
        .call(
            "ModifyCacheParameterGroup",
            &params(&[
                ("CacheParameterGroupName", &group),
                (
                    "ParameterNameValues.ParameterNameValue.1.ParameterName",
                    "slowlog-log-slower-than",
                ),
                (
                    "ParameterNameValues.ParameterNameValue.1.ParameterValue",
                    threshold,
                ),
                (
                    "ParameterNameValues.ParameterNameValue.2.ParameterName",
                    "slowlog-max-len",
                ),
                (
                    "ParameterNameValues.ParameterNameValue.2.ParameterValue",
                    depth,
                ),
            ]),
        )
        .await;
    assert_eq!(
        modified.status, 200,
        "case {case}: the slow-log parameters must be set, got {} {}",
        modified.status, modified.body
    );
    group
}

/// Create that parameter group, then a cluster running under it, and answer a RESP session on
/// the address it published.
///
/// `pub(crate)` for the reason [`recording_parameter_group`] is.
pub(crate) async fn session_on(
    emulator: &Emulator,
    case: &str,
    threshold: &str,
    depth: &str,
) -> Session {
    let group = recording_parameter_group(emulator, case, threshold, depth).await;
    let created = emulator
        .call(
            "CreateCacheCluster",
            &crate::harness::cluster_create_params(
                case,
                &[
                    ("Engine", "redis"),
                    ("NumCacheNodes", "1"),
                    ("CacheParameterGroupName", &group),
                ],
            ),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case}: the cluster must be created, got {} {}",
        created.status, created.body
    );
    let described = poll_until(
        emulator,
        REGION,
        case,
        "the cluster reaching available with its node provisioned",
        "DescribeCacheClusters",
        &params(&[("CacheClusterId", case), ("ShowCacheNodeInfo", "true")]),
        |reply| reply.text("CacheCluster/CacheClusterStatus") == Some("available"),
    )
    .await;
    let (address, port) = endpoint_of(
        &described,
        "CacheNodes/CacheNode/Endpoint/Address",
        "CacheNodes/CacheNode/Endpoint/Port",
        case,
    );
    open(case, &address, &port).await
}

/// The entries `SLOWLOG GET` reports, each as the arguments of the command it recorded.
///
/// `pub(crate)` for the reason [`recording_parameter_group`] is.
pub(crate) async fn recorded(case: &str, session: &mut Session) -> Vec<Vec<String>> {
    // `-1` asks for every entry the log holds rather than one page of them.
    let answered = command(case, session, &["SLOWLOG", "GET", "-1"]).await;
    let Resp::Array(entries) = answered else {
        panic!("case {case}: SLOWLOG GET must answer an array, got {answered:?}");
    };
    entries
        .into_iter()
        .map(|entry| {
            let Resp::Array(fields) = entry else {
                panic!("case {case}: a slow-log entry must be an array, got {entry:?}");
            };
            assert_eq!(
                fields.len(),
                6,
                "case {case}: an entry carries its id, instant, duration, arguments, client \
                 address and client name, got {fields:?}"
            );
            for (at, what) in [(0, "id"), (1, "instant"), (2, "duration")] {
                assert!(
                    matches!(fields[at], Resp::Integer(value) if value >= 0),
                    "case {case}: an entry's {what} must be a number, got {:?}",
                    fields[at]
                );
            }
            let Resp::Array(ref arguments) = fields[3] else {
                panic!(
                    "case {case}: an entry's arguments must be an array, got {:?}",
                    fields[3]
                );
            };
            arguments
                .iter()
                .map(|argument| match argument {
                    Resp::Bulk(text) => text.clone(),
                    other => panic!("case {case}: an argument must be a bulk string, got {other:?}"),
                })
                .collect()
        })
        .collect()
}

#[tokio::test]
async fn a_command_is_recorded_when_it_reaches_the_threshold_the_parameter_group_states() {
    let emulator = Emulator::new();
    // The axis: the threshold the cache's own parameter group states, and so whether a
    // command the client sends is one the engine records at all.
    let cases: [(&str, &str, bool); 2] = [
        ("slowlog-every-command", EVERY_COMMAND, true),
        ("slowlog-no-command", NO_COMMAND, false),
    ];
    for (case, threshold, records) in cases {
        let mut session = session_on(&emulator, case, threshold, "128").await;
        assert_eq!(
            command(case, &mut session, &["SET", "probe", "a value"]).await,
            Resp::Status("OK".to_string()),
            "case {case}: the cache must take the write whatever its slow log does"
        );
        assert_eq!(
            command(case, &mut session, &["GETRANGE", "probe", "0", "-1"]).await,
            Resp::Bulk("a value".to_string()),
            "case {case}: the cache must answer the whole of what the write stored"
        );
        let entries = recorded(case, &mut session).await;
        let written = vec![
            "SET".to_string(),
            "probe".to_string(),
            "a value".to_string(),
        ];
        if !records {
            assert!(
                entries.is_empty(),
                "case {case}: a command that did not reach the threshold must not be recorded, \
                 got {entries:?}"
            );
            assert_eq!(
                command(case, &mut session, &["SLOWLOG", "LEN"]).await,
                Resp::Integer(0),
                "case {case}: a slow log with no entry has length zero"
            );
            continue;
        }
        assert!(
            entries.contains(&written),
            "case {case}: the command the engine executed must be recorded with its own \
             arguments, got {entries:?}"
        );
        assert_eq!(
            entries.first().map(|arguments| arguments.first().cloned()),
            Some(Some("GETRANGE".to_string())),
            "case {case}: SLOWLOG GET answers the most recent command first, got {entries:?}"
        );
        // A reset discards what the log held.
        assert_eq!(
            command(case, &mut session, &["SLOWLOG", "RESET"]).await,
            Resp::Status("OK".to_string()),
            "case {case}: SLOWLOG RESET must be served"
        );
        let after = recorded(case, &mut session).await;
        assert!(
            !after.contains(&written),
            "case {case}: a reset must discard the entries the log held, got {after:?}"
        );
    }
}

#[tokio::test]
async fn the_slow_log_keeps_at_most_the_entries_its_depth_parameter_states() {
    let emulator = Emulator::new();
    let case = "slowlog-depth";
    let depth = 2;
    let mut session = session_on(&emulator, case, EVERY_COMMAND, &depth.to_string()).await;
    let keys: Vec<String> = (1..=5).map(|index| format!("depth-{index}")).collect();
    for key in &keys {
        assert_eq!(
            command(case, &mut session, &["SET", key, "kept"]).await,
            Resp::Status("OK".to_string()),
            "case {case}: the cache must take the write of {key}"
        );
    }
    let entries = recorded(case, &mut session).await;
    assert_eq!(
        entries.len(),
        depth,
        "case {case}: the log keeps what its depth parameter states, got {entries:?}"
    );
    // The last writes, newest first: the depth drops the OLDEST entries.
    assert_eq!(
        entries
            .iter()
            .map(|arguments| arguments.get(1).cloned().unwrap_or_default())
            .collect::<Vec<String>>(),
        vec![keys[4].clone(), keys[3].clone()],
        "case {case}: the entries kept are the most recent commands, got {entries:?}"
    );
}

#[tokio::test]
async fn getrange_and_setrange_read_and_write_the_bytes_of_a_value() {
    let emulator = Emulator::new();
    let case = "slowlog-string-ranges";
    let mut session = session_on(&emulator, case, EVERY_COMMAND, "128").await;
    // The axis: the command, and the reply the engine answers it with — the offsets a range
    // names (a whole value, a prefix, a suffix, one byte, past each end, an empty range), a
    // write inside the value, a write past its end (padded with zero bytes), an empty write
    // and an offset before the start of the value.
    let cases: &[(&str, &[&str], Resp)] = &[
        ("set", &["SET", "ranged", "This is a string"], Resp::Status("OK".to_string())),
        ("the whole value", &["GETRANGE", "ranged", "0", "-1"], Resp::Bulk("This is a string".to_string())),
        ("a prefix", &["GETRANGE", "ranged", "0", "3"], Resp::Bulk("This".to_string())),
        ("a suffix", &["GETRANGE", "ranged", "-6", "-1"], Resp::Bulk("string".to_string())),
        ("one byte", &["GETRANGE", "ranged", "5", "5"], Resp::Bulk("i".to_string())),
        ("past the end", &["GETRANGE", "ranged", "10", "100"], Resp::Bulk("string".to_string())),
        ("past the start", &["GETRANGE", "ranged", "-100", "3"], Resp::Bulk("This".to_string())),
        ("an empty range", &["GETRANGE", "ranged", "6", "3"], Resp::Bulk(String::new())),
        ("a key that is not there", &["GETRANGE", "ghost", "0", "-1"], Resp::Bulk(String::new())),
        ("a write inside the value", &["SETRANGE", "ranged", "0", "That"], Resp::Integer(16)),
        ("read back", &["GETRANGE", "ranged", "0", "-1"], Resp::Bulk("That is a string".to_string())),
        ("a write past the end", &["SETRANGE", "fresh", "2", "tail"], Resp::Integer(6)),
        ("the padding", &["GETRANGE", "fresh", "0", "-1"], Resp::Bulk("\0\0tail".to_string())),
        ("an empty write", &["SETRANGE", "ranged", "0", ""], Resp::Integer(16)),
        ("an empty write on a free key", &["SETRANGE", "untouched", "0", ""], Resp::Integer(0)),
        ("the key an empty write did not create", &["EXISTS", "untouched"], Resp::Integer(0)),
        (
            "an offset before the start",
            &["SETRANGE", "ranged", "-1", "x"],
            Resp::Error("ERR offset is out of range".to_string()),
        ),
    ];
    for (what, sent, expected) in cases {
        let answered = command(case, &mut session, sent).await;
        assert_eq!(
            answered, *expected,
            "case {case}/{what}: expected {expected:?}, got {answered:?}"
        );
    }
}
