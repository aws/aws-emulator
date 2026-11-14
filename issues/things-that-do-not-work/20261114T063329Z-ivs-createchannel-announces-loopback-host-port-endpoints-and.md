Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-channel-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-channel-lifecycle ivs-viewer-session-revocation
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs|CreateChannel|2xx->2xx|$.channel.ingestEndpoint
Issue-Key: ivs|CreateChannel|2xx->2xx|$.channel.playbackUrl
Issue-Related: 20261103T113752Z-ivs-createchannel-srt-block-mints-an-endpoint-and.md
Issue-Commit: d0b60cf4e620490598360e9e180ae29d3f253b9d

# IVS CreateChannel announces loopback host:port endpoints and an out-of-shape SRT passphrase

> _Note: this issue was identified against the code base at commit `d0b60cf4e620490598360e9e180ae29d3f253b9d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
Replaying the wire corpus (simparity-wire-aws-v1) against the IVS emulator (service token `ivs`, crate services/service-com-amazonaws-ivs-amazoninteractivevideoservice) shows CreateChannel returning a channel whose four GENERATED members all fall outside the shape the recording holds for them. The corpus does not pin these values literally — they are per-run hosts and secrets, reported as `<shaped-not-asserted>` — it pins their SHAPE CLASS, and all four fail that check. Status and the rest of the channel projection are correct (expected 200, got 200). TWO causes, both grounded in the same representative step (fixture ivs-channel-lifecycle, step task#1; the cluster also covers ivs-viewer-session-revocation#setup#1, so both fixtures close with one fix). A fixer must close BOTH before submitting.

Cause 1 of 2 — the three announced endpoints (`$.channel.ingestEndpoint`, `$.channel.playbackUrl`, `$.channel.srt.endpoint`) are a bare loopback IP with a port instead of a DNS hostname.
AWS: recorded behavior only. Smithy (com.amazonaws.ivs, restJson1): `Channel.ingestEndpoint` -> `IngestEndpoint`, `Channel.playbackUrl` -> `PlaybackURL`, `Srt.endpoint` -> `SrtEndpoint` are all plain strings with NO @length and NO @pattern, so the model states no form and the recording is the only authority. What the recording establishes is a shape class, not a literal: the ingest endpoint is a DNS HOSTNAME — a minted lowercase-hex label followed by a service-owned contribute domain — carrying NO scheme and NO port, and the SRT endpoint is the SAME host as `ingestEndpoint` (corroborated inside this corpus by ivs-batch-get-channel-and-stream-key#task#3, where `$.channels[0].ingestEndpoint` and `$.channels[0].srt.endpoint` are one identical host). The playback URL is an `https://` URL whose HOST is a hostname of that same class and whose path is the recorded `/api/video/v1/<region>.<account>.channel.<channel-id>.m3u8` layout. The hex label, region, account id and channel id are per-run values that fill the template; the template — hostname form, no port, scheme and path layout — is the requirement.
Emulator: the replay reports all three as shape mismatches. `ingestEndpoint` and `srt.endpoint` come back as a bare loopback IP literal with a `:<port>` suffix and no domain label at all, and `playbackUrl` comes back as `https://<loopback-ip>:<port>/api/video/v1/...` — the recorded PATH layout is preserved but the host is the emulator's own listener address rather than a hostname of the recorded class. create_channel.rs is explicit that this is deliberate: it stores `ingest_endpoint: None`, `playback_url: None` and `Srt.endpoint: None` and defers all three to a shared projection (`crate::state::announced_channel`) that composes "the DEPLOYMENT's address" for each request that reports the channel. The specific IP and port are per-run and are NOT the finding; the finding is that an address of that FORM (bare IP, explicit port, no domain) is a different shape class from the hostname AWS issues.
Why: a client that consumes the channel's contribute address does not use it as an opaque string. It composes `rtmps://<ingestEndpoint>:443/app/<streamKey>` or `srt://<srt.endpoint>` — appending its own port — so an endpoint that already carries `:<port>` yields a doubly-ported, unusable URL; code that splits the host on `.` to recover the channel label, or that validates it as a hostname / stores it in a hostname-typed field, rejects a bare IP outright. For `playbackUrl` a player that parses the host to pick a CDN or to assert TLS-by-hostname mis-handles it. All of this works against AWS and breaks against this emulator.
Fix: the shared channel projection that composes these three members — `crate::state::announced_channel` in services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/state.rs, which create_channel.rs:250-265 delegates to — not the serializer and not the store (the members are never persisted). It must announce an address of AWS's recorded shape class: a hostname label plus domain for the ingest/SRT endpoint (one string shared by both members) and that same class of host inside the `https://.../api/video/v1/<region>.<account>.channel.<id>.m3u8` playback URL, with no port component. Note the tension with 20261109T213051Z-ivs-realtime-stage-responses-announce-upstream-live-video.md (ivs-realtime announcing upstream endpoints): whatever reachability scheme is chosen, the member's announced FORM still has to be AWS's, since the corpus asserts the form.

Cause 2 of 2 — `$.channel.srt.passphrase` is minted outside the recorded alphabet/width.
AWS: recorded behavior only. Smithy: `Srt.passphrase` -> `SrtPassphrase`, a plain @sensitive string with no @length and no @pattern, documented as the "Auto-generated passphrase to enable encryption", applicable only when `insecureIngest` is not enabled — so the model states no width or alphabet and the recording is the only authority. The recording carries an auto-generated secret whose alphabet and WIDTH are what the shape assertion pins; the secret's text is per-run and is not a requirement.
Emulator: the replay reports `$.channel.srt.passphrase` as a shape mismatch. The value is a 32-character mixed-case ASCII-alphanumeric string produced by `crate::state::mint_srt_passphrase` over `SRT_PASSPHRASE_ALPHABET` with `SRT_PASSPHRASE_LEN = 32`, a width the code's own comment says was CHOSEN because "the model states no width for SrtPassphrase" rather than taken from AWS — and it does not match the recorded shape.
Why: this is the only generated secret whose shape is observable in the channel response. A client that length-checks or character-class-validates the passphrase before handing it to an SRT encoder, or that stores it in a fixed-width field, accepts what AWS issues and rejects what this emulator issues.
Fix: the secret generator in crate::state — `mint_srt_passphrase` / `SRT_PASSPHRASE_ALPHABET` / `SRT_PASSPHRASE_LEN` in services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/state.rs, called from create_channel.rs:237-242. No other operation mints an `Srt` block.

Traces: ivs-channel-lifecycle#task#1 (representative, both causes) and ivs-viewer-session-revocation#setup#1 (same cluster); recorded endpoint shape corroborated by ivs-batch-get-channel-and-stream-key#task#3.

Related open issues, NONE of which owns this cluster (the replay reports owned_by [] for it): 20261103T113752Z-ivs-createchannel-srt-block-mints-an-endpoint-and.md covers only the two `$.channel.srt.*` keys for this same step and does not carry this cluster's replay key, which now also spans `$.channel.ingestEndpoint` and `$.channel.playbackUrl` — the emulator's endpoint form has since drifted from a hostname to a loopback IP:port, so that issue's recorded ACTUAL values no longer describe what the emulator serves; and 20261109T213051Z-ivs-realtime-stage-responses-announce-upstream-live-video.md is the same class of defect (announced endpoint form) in ivs-realtime.

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true ("/__admin/health answered 200 and the :36815 data listener accepts"), steps_total 84, passed 58, mismatched 21, blocked 5, errored 0, score 69.0%, divergence_count 13, bootstrap_signal false.

The one net-new cluster (every other cluster in this replay carries a non-empty owned_by):
  divergence_key: service=ivs|CreateChannel|status=200->200|keys=$.channel.ingestEndpoint,$.channel.playbackUrl,$.channel.srt.endpoint,$.channel.srt.passphrase
  verdict: mismatch, owned_by: [], stub_path: ""
  expected_status 200, actual_status 200 (status is CORRECT)
  representative: ivs-channel-lifecycle#task#1; affected_count 2, other member ivs-viewer-session-revocation#setup#1
  detail:
    $.channel.ingestEndpoint: value_mismatch (expected '<shaped-not-asserted>', got '127.0.0.1:35353')
    $.channel.playbackUrl: value_mismatch (expected '<shaped-not-asserted>', got 'https://127.0.0.1:35353/api/video/v1/us-west-2.000000000000.channel.96b292210807.m3u8')
    $.channel.srt.endpoint: value_mismatch (expected '<shaped-not-asserted>', got '127.0.0.1:35353')
    $.channel.srt.passphrase: value_mismatch (expected '<shaped-not-asserted>', got 'YhQO0nZGKWp4nsQbPnG4W0gKpI8H90bv')

The literal values above are ONE run's output and are NOT themselves requirements: the corpus marks all four members shaped-not-asserted, the port, the channel id '96b292210807', the account '000000000000' and the secret text change every run, and the mismatch persists across runs. What diverges is the shape class — bare IP + explicit port where a hostname with no port is recorded (three endpoint members), and a 32-char mixed-case alphanumeric secret where the recorded secret shape is a different one.

Recorded shape corroboration from the same corpus: ivs-batch-get-channel-and-stream-key#task#3 records $.channels[0].ingestEndpoint and $.channels[0].srt.endpoint as one identical DNS host (a lowercase-hex label under a service-owned contribute domain, no port).

Smithy (services/service-com-amazonaws-ivs-amazoninteractivevideoservice/smithy-model/com-amazonaws-ivs-amazoninteractivevideoservice.json): Channel.ingestEndpoint -> IngestEndpoint (string, no constraints); Channel.playbackUrl -> PlaybackURL (string, no constraints); Srt.endpoint -> SrtEndpoint (string, no constraints); Srt.passphrase -> SrtPassphrase (string, @sensitive, no constraints) — hence "recorded behavior only" for all four.

Emulator source backing the actual values: src/ops/create_channel.rs lines 231-266 — `ingest_endpoint: None`, `playback_url: None` and `Srt { endpoint: None, passphrase: Some(srt_passphrase) }`, with the comment at lines 250-253 stating both announced addresses "are this emulator's own, composed for the request that reports the channel by the shared projection below"; the passphrase at lines 237-242 from `crate::state::mint_srt_passphrase` (SRT_PASSPHRASE_ALPHABET / SRT_PASSPHRASE_LEN = 32 in src/state.rs).

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), or exercise it by hand against the data listener in region us-west-2:
1. POST /CreateChannel with a body naming only `name` (fixture ivs-channel-lifecycle#task#1; ivs-viewer-session-revocation#setup#1 issues the same call in its setup phase).
2. Read `$.channel.ingestEndpoint`, `$.channel.playbackUrl`, `$.channel.srt.endpoint` and `$.channel.srt.passphrase` from the 200 response.
3. Cause 1: observe that `ingestEndpoint` and `srt.endpoint` are one bare loopback IP carrying an explicit `:<port>` and no domain label, and that `playbackUrl`'s host is that same IP:port while its path keeps the recorded `/api/video/v1/<region>.<account>.channel.<id>.m3u8` layout. The corpus records these as a DNS hostname with no port (confirmed by ivs-batch-get-channel-and-stream-key#task#3, where ingestEndpoint and srt.endpoint are one identical hostname); the replay reports all three as shape mismatches.
4. Cause 2: observe that `srt.passphrase` is a 32-character mixed-case ASCII-alphanumeric secret, outside the recorded passphrase shape; the replay reports it as a fourth shape mismatch.
5. Re-run: the port, channel id and secret text all change and every one of the four mismatches persists — the per-run values are not the divergence, the shape class is.
