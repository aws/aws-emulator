Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-batch-get-channel-and-stream-key
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-batch-get-channel-and-stream-key ivs-channel-lifecycle ivs-viewer-session-revocation
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs|BatchGetChannel|2xx->2xx|$.channels[*].ingestEndpoint
Issue-Key: ivs|BatchGetChannel|2xx->2xx|$.channels[*].playbackUrl
Issue-Key: ivs|BatchGetChannel|2xx->2xx|$.channels[*].srt.endpoint
Issue-Key: ivs|CreateChannel|2xx->2xx|$.channel.playbackUrl
Issue-Key: ivs|GetChannel|2xx->2xx|$.channel.playbackUrl
Issue-Key: ivs|UpdateChannel|2xx->2xx|$.channel.playbackUrl
Issue-Related: 20261103T113752Z-ivs-createchannel-srt-block-mints-an-endpoint-and.md
Issue-Commit: afd3b182fafbbe8cee1c009b5b1f0d78bd466684

# IVS channel projections announce upstream live-video.net hosts and derive ingest endpoints per channel

> _Note: this issue was identified against the code base at commit `afd3b182fafbbe8cee1c009b5b1f0d78bd466684`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetChannel

## Description
Five causes, found by replaying the wire corpus simparity-wire-aws-v1 against the emulator built from this checkout (5 net-new divergence clusters, all service=ivs, all verdict=mismatch, all with empty stub_path and empty owned_by). The shared fault is the Channel structure's endpoint projection: every operation that returns a Channel (CreateChannel, GetChannel, UpdateChannel, BatchGetChannel) emits `ingestEndpoint`, `srt.endpoint` and `playbackUrl` as constant upstream Amazon IVS production hostnames rather than as addresses the emulator is actually reached on, and it builds all of them from ONE per-channel minted id, so a channel's contribute (ingest/SRT) host and its playback host are always the same label — which is not how the recording shows them. All five causes are ivs and all five must be closed before this issue is submitted.

Cause 1 of 5 — BatchGetChannel projects each channel's ingestEndpoint/srt.endpoint from a per-channel id instead of the account+region contribute host, and its playbackUrls name a host the emulator does not serve.
AWS: Smithy (com.amazonaws.ivs, restJson1): BatchGetChannel (POST /BatchGetChannel, 200) output `channels` -> Channels of `Channel`, whose members `ingestEndpoint` -> ChannelIngestEndpoint (plain string, no @length/@pattern, documented "Channel ingest endpoint, part of the definition of an ingest server, used when you set up streaming software"), `playbackUrl` -> PlaybackURL (plain string, documented "Channel playback URL") and `srt` -> Srt{endpoint -> SrtEndpoint, passphrase -> SrtPassphrase}. The model states no host form for any of them, so this is recorded behavior only. What the recording establishes: within one account+region the two channels returned in the same BatchGetChannel response carry the SAME contribute host in `ingestEndpoint` (and in `srt.endpoint`, which equals it) while their `playbackUrl` hosts DIFFER per channel — in the recording channel `f2b3e161ff89` has contribute host `<id-A>.global-contribute.live-video.net` and playback host `<id-B>.us-west-2.playback.live-video.net`, two different labels, and the second channel's `ingestEndpoint` is the same `<id-A>` host (it is the member that matched). Hypothesis: AWS's ingest/contribute endpoint is an account+region-level host shared by every channel, and only the playback host is per-channel. The requirement that is NOT a per-run value is the relation: a channel's contribute host is independent of its playback host and is shared across the account's channels; the hex labels themselves are per-run and are not requirements. In addition, both `playbackUrl` values are reported `host_not_own`: whatever the label, the host a client is sent to must be the address that actually serves that channel's playback for the caller — on AWS the IVS edge, on an emulator its own data listener, because the emulator is the thing serving it.
Emulator: answers 200 with the right channels and the right channel ids, but `$.channels[0].ingestEndpoint` and `$.channels[0].srt.endpoint` are value mismatches (the emulator serves `<the channel's own playback label>.global-contribute.live-video.net` where the recording holds the account's shared contribute host), and `$.channels[0].playbackUrl` and `$.channels[1].playbackUrl` are reported `host_not_own` — the URLs point at `*.us-west-2.playback.live-video.net`, an upstream Amazon domain the emulator does not answer on. The source is explicit: src/ops/create_channel.rs mints one `ingest_id` per channel and formats both `format!("{ingest_id}.global-contribute.live-video.net")` (reused verbatim as `Srt.endpoint`) and `https://{ingest_id}.{region}.playback.live-video.net/...` from it, so ingest is per-channel and both hosts are upstream constants; reads simply echo the stored strings.
Why: a client does what these members tell it to. Pointed at `*.playback.live-video.net` it leaves the sandbox — it either egresses to real AWS unauthenticated and fails late in its media path with a transport error naming no emulator, or cannot resolve the host at all, while the API call that handed out the value looked healthy. And a client or test that caches one contribute endpoint for the account (the documented ingest-server setup) sees a different ingest host per channel, so a second channel's encoder configuration is wrong.
Fix: the Channel endpoint construction in the CreateChannel handler plus the shared state helper the other three read back (services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/create_channel.rs and src/state.rs): derive one account+region contribute host for the store and build `playbackUrl` from the emulator's own advertised host and port for the request, keeping the per-channel playback label and the path shape, instead of formatting constant upstream IVS domains from a single per-channel id.
Traces: ivs-batch-get-channel-and-stream-key#task#3.

Cause 2 of 5 — the same BatchGetChannel defect on the single-ARN read later in the same fixture.
AWS: as Cause 1; the same Channel projection is required to be self-consistent across a batch read and a single-channel read.
Emulator: `$.channels[0].ingestEndpoint` and `$.channels[0].srt.endpoint` value-mismatch the recorded contribute host and `$.channels[0].playbackUrl` is `host_not_own`; status 200 is correct.
Why: same as Cause 1 — the value is wrong on every read path, so there is no operation a client can use to recover a usable ingest or playback address.
Fix: same component as Cause 1; it is one projection, so one fix closes both BatchGetChannel clusters.
Traces: ivs-batch-get-channel-and-stream-key#task#4.

Cause 3 of 5 — CreateChannel hands out the upstream playback host at creation time, and mints an `Srt` block outside the recorded shape.
AWS: Smithy: CreateChannel (POST /CreateChannel, 200) output `channel` -> Channel, `streamKey` -> StreamKey. `Srt.endpoint` -> SrtEndpoint and `Srt.passphrase` -> SrtPassphrase are plain strings (passphrase @sensitive) with no @length/@pattern, so recorded behavior only: the corpus does not assert these two literally (`<shaped-not-asserted>`) but asserts their shape class — the endpoint's host form (the account's contribute host, `<label>.global-contribute.live-video.net`, the same value as `ingestEndpoint`) and the passphrase's alphabet and width. `playbackUrl` carries the Cause 1 ownership requirement.
Emulator: answers 200, but the replay reports `$.channel.srt.endpoint` and `$.channel.srt.passphrase` as shape mismatches (a 12-lowercase-hex contribute host minted per channel, and a 32-character mixed-case alphanumeric secret cut by `crate::state::mint_srt_passphrase` over `SRT_PASSPHRASE_ALPHABET`/`SRT_PASSPHRASE_LEN` — a width src/state.rs comments as chosen because "the model states no width for SrtPassphrase", i.e. chosen rather than taken from AWS), and `$.channel.playbackUrl` as `host_not_own`.
Why: this is the response a client caches at creation, so the bad playback host and an off-shape ingest secret propagate into every encoder and player configuration built from it; a client that length-checks or parses the passphrase, or that splits the announced host, rejects or mis-handles what AWS would have given it.
Fix: the CreateChannel handler's srt/endpoint minting and the state helper that generates the secret (src/ops/create_channel.rs's `ingest_endpoint`/`Srt { endpoint, passphrase }` construction, and src/state.rs's `mint_srt_passphrase`/`SRT_PASSPHRASE_ALPHABET`/`SRT_PASSPHRASE_LEN`), together with the own-host endpoint builder of Cause 1.
Traces: ivs-channel-lifecycle#task#1; the same cluster also covers ivs-viewer-session-revocation#setup#1, so both close together. The srt half overlaps open issue 20261103T113752Z-ivs-createchannel-srt-block-mints-an-endpoint-and.md, but the replay reports this cluster with owned_by empty — the cluster's key now also covers `$.channel.playbackUrl`, so that issue does not carry this cluster's replay key and does not close it.

Cause 4 of 5 — GetChannel reads the upstream playback host back.
AWS: Smithy: GetChannel (POST /GetChannel, 200, readonly) output `channel` -> Channel; the recording holds `playbackUrl` as an absolute URL on the host that serves that channel's playback for the caller. Fixed text: the https scheme and the `/api/video/v1/<region>.<account>.channel.<channel-id>.m3u8` path shape; variable: the serving host and port, the region, the account id and the channel id, taken from the deployment and the channel's state.
Emulator: answers 200 with an otherwise correct channel, and `$.channel.playbackUrl` is reported `host_not_own` — the stored `*.us-west-2.playback.live-video.net` URL.
Why: GetChannel is the normal way a player resolves a channel's playback URL, so this is the path by which a client is routed out of the emulator to real AWS; the manifest fetch fails outside the API call that produced the value.
Fix: the GetChannel projection must read the channel's endpoints through the same own-host builder as Cause 1 (store the label and compose the URL at projection time, or rewrite the host on read) — src/ops/get_channel.rs plus src/state.rs.
Traces: ivs-channel-lifecycle#task#2; the cluster also covers ivs-channel-lifecycle#task#5 and ivs-viewer-session-revocation#task#5.

Cause 5 of 5 — UpdateChannel re-emits the same upstream playback host.
AWS: Smithy: UpdateChannel (POST /UpdateChannel, 200) output `channel` -> Channel; an update returns the channel's current projection, with `playbackUrl` under the same ownership requirement as Cause 4.
Emulator: answers 200 and applies the update, but `$.channel.playbackUrl` is again `host_not_own`.
Why: a client that re-reads the channel after an update refreshes its cached playback URL from this response, so the unusable host survives every update.
Fix: same component as Cause 4 — the UpdateChannel handler's Channel projection (src/ops/update_channel.rs) through the shared own-host endpoint builder.
Traces: ivs-channel-lifecycle#task#4.

Related open issues, none of which carries any of these clusters' replay keys: 20261103T113752Z-ivs-createchannel-srt-block-mints-an-endpoint-and.md (the srt half of Cause 3), 20261109T213051Z-ivs-realtime-stage-responses-announce-upstream-live-video.md (the same CLASS of defect — upstream live-video.net hosts announced in endpoint members — in service ivs-realtime), and the ListChannels `failoverSlateConfigurationArn` issues, which concern a different member of the channel summary.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true (/__admin/health 200 and the :40305 data listener accepts), 15 fixtures replayed, 84 steps, 52 passed, 27 mismatched, 0 errored, 5 blocked, score 61.9%, divergence_count=17. Twelve clusters carry owned_by (skipped); the five below are net-new (stub_path empty, owned_by empty, all service=ivs, all verdict=mismatch, expected_status 200 / actual_status 200).

Cluster A (representative) — fixture ivs-batch-get-channel-and-stream-key, step task#3, operation BatchGetChannel, affected_count 1.
divergence_key: service=ivs|BatchGetChannel|status=200->200|keys=$.channels[0].ingestEndpoint,$.channels[0].playbackUrl,$.channels[0].srt.endpoint,$.channels[1].playbackUrl
detail:
  $.channels[0].ingestEndpoint: value_mismatch (expected '45b9b6f79c8b.global-contribute.live-video.net', got 'cd955b1f987b.global-contribute.live-video.net')
  $.channels[0].srt.endpoint: value_mismatch (same two values)
  $.channels[0].playbackUrl: host_not_own ('https://cd955b1f987b.us-west-2.playback.live-video.net/api/video/v1/us-west-2.000000000000.channel.f2b3e161ff89.m3u8')
  $.channels[1].playbackUrl: host_not_own ('https://45b9b6f79c8b.us-west-2.playback.live-video.net/api/video/v1/us-west-2.000000000000.channel.ca21abb24ec1.m3u8')
Note on reading this: the recording's channel[0] has contribute label 45b9b6f79c8b and playback label cd955b1f987b — two DIFFERENT labels — while channel[1]'s ingestEndpoint matched (it is not in the key), i.e. the recording gives both channels the SAME contribute host. The emulator instead gives channel[0] the contribute label equal to its own playback label. The hex labels are per-run values and not requirements; what diverges is that relation, plus host ownership of the playback URLs (expected and actual text are identical there — the verdict is that the announced host is not one the emulator answers on).

Cluster B — fixture ivs-batch-get-channel-and-stream-key, step task#4, operation BatchGetChannel, affected_count 1.
divergence_key: service=ivs|BatchGetChannel|status=200->200|keys=$.channels[0].ingestEndpoint,$.channels[0].playbackUrl,$.channels[0].srt.endpoint
detail: ingestEndpoint and srt.endpoint value_mismatch ('45b9b6f79c8b.global-contribute.live-video.net' expected, 'cd955b1f987b.global-contribute.live-video.net' served); $.channels[0].playbackUrl host_not_own.

Cluster C — fixture ivs-channel-lifecycle, step task#1, operation CreateChannel, affected_count 2 (other member ivs-viewer-session-revocation#setup#1).
divergence_key: service=ivs|CreateChannel|status=200->200|keys=$.channel.playbackUrl,$.channel.srt.endpoint,$.channel.srt.passphrase
detail:
  $.channel.srt.endpoint: value_mismatch (expected '<shaped-not-asserted>', got '7a82b3161f0d.global-contribute.live-video.net')
  $.channel.srt.passphrase: value_mismatch (expected '<shaped-not-asserted>', got a 32-char mixed-case alphanumeric secret)
  $.channel.playbackUrl: host_not_own ('https://7a82b3161f0d.us-west-2.playback.live-video.net/api/video/v1/us-west-2.000000000000.channel.952d76ab8b22.m3u8')
The two srt values are shaped-not-asserted: the per-run hex label and the per-run secret text are not requirements — the shape class is (contribute-host form equal to ingestEndpoint; passphrase alphabet and width).

Cluster D — fixture ivs-channel-lifecycle, step task#2, operation GetChannel, affected_count 3 (ivs-channel-lifecycle#task#5, ivs-viewer-session-revocation#task#5).
divergence_key: service=ivs|GetChannel|status=200->200|keys=$.channel.playbackUrl
detail: $.channel.playbackUrl: host_not_own (the stored 'https://7a82b3161f0d.us-west-2.playback.live-video.net/...m3u8' URL; expected text identical, so the divergence is host ownership only).

Cluster E — fixture ivs-channel-lifecycle, step task#4, operation UpdateChannel, affected_count 1.
divergence_key: service=ivs|UpdateChannel|status=200->200|keys=$.channel.playbackUrl
detail: $.channel.playbackUrl: host_not_own (same URL as Cluster D).

Expected (AWS): a Channel's `playbackUrl` is an absolute https URL on the host that actually serves that channel's playback for the caller — the emulator's own advertised host and port — with the `/api/video/v1/<region>.<account>.channel.<channel-id>.m3u8` path shape fixed and host/port/region/account/channel-id taken from the deployment and state; a Channel's `ingestEndpoint` and `srt.endpoint` are one contribute host shared by every channel in the account+region and independent of the channel's playback label; `srt.passphrase` is an auto-generated secret of AWS's shape class.
Actual (emulator): one per-channel minted id supplies BOTH the contribute host and the playback host, and both are constant upstream Amazon production domains (`*.global-contribute.live-video.net`, `*.us-west-2.playback.live-video.net`) that the emulator does not answer on; the passphrase is a chosen 32-char mixed-case alphanumeric. Statuses (200) and all other channel members match.
Emulator source backing the actual values: src/ops/create_channel.rs lines 231-261 (`mint_hex("ingest", 6)`, `format!("{ingest_id}.global-contribute.live-video.net")` reused as `Srt.endpoint`, and the `https://{ingest_id}.{region}.playback.live-video.net/...` playback URL), and src/state.rs lines 459-481 (`SRT_PASSPHRASE_ALPHABET`, `SRT_PASSPHRASE_LEN = 32`, `mint_srt_passphrase`).

## Steps to Reproduce
1. Clone this repo, boot the emulator built from it, and replay trace set simparity-wire-aws-v1 (replay_wire_corpus), or drive the data listener by hand in region us-west-2.
2. Causes 1-2: create two channels, then POST /BatchGetChannel with both ARNs (fixture ivs-batch-get-channel-and-stream-key, steps task#3 and task#4). In the 200 body compare `$.channels[*].ingestEndpoint` / `$.channels[*].srt.endpoint`: the emulator gives each channel a DIFFERENT contribute host, equal to that channel's own playback label, where the recording gives both channels the same contribute host, distinct from either playback label. Then assert that the host of every `$.channels[*].playbackUrl` equals the host:port the emulator is being reached on — it is `*.us-west-2.playback.live-video.net` instead.
3. Cause 3: POST /CreateChannel naming only `name` (fixture ivs-channel-lifecycle#task#1; ivs-viewer-session-revocation#setup#1 does the same in setup). Read `$.channel.srt.endpoint`, `$.channel.srt.passphrase` and `$.channel.playbackUrl`: the srt endpoint is a per-channel 12-hex contribute host, the passphrase is a 32-char mixed-case alphanumeric, and the playback URL names an upstream host. Re-running changes the hex label and the secret text and the divergence persists — the labels are not the finding.
4. Cause 4: POST /GetChannel for that ARN (fixture ivs-channel-lifecycle#task#2, also task#5 and ivs-viewer-session-revocation#task#5) and assert host ownership of `$.channel.playbackUrl`.
5. Cause 5: POST /UpdateChannel for that ARN (fixture ivs-channel-lifecycle#task#4) and assert host ownership of `$.channel.playbackUrl` in the returned channel.
6. After the fix, re-replay all five steps (ivs-batch-get-channel-and-stream-key#task#3 and #task#4, ivs-channel-lifecycle#task#1, #task#2 and #task#4): every announced endpoint must resolve to the emulator's own listener, the contribute host must be one shared account+region host independent of the playback label, and the srt passphrase must match AWS's shape class.
