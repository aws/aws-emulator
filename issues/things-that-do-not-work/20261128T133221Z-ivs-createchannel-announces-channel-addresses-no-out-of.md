Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-channel-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-channel-lifecycle ivs-viewer-session-revocation
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs|CreateChannel|2xx->2xx|$.channel.srt.endpoint
Issue-Related: 20261125T153458Z-ivs-5-net-new-causes-announced-channel-addresses.md
Issue-Commit: ff49842cf7656b5a010943ef9d0fd100550d134a

# IVS: CreateChannel announces channel addresses no out-of-process client can use — ingestEndpoint/srt.endpoint are a bare IP:port and playbackUrl a host the emulator does not serve

> _Note: this issue was identified against the code base at commit `ff49842cf7656b5a010943ef9d0fd100550d134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
Replaying the wire corpus (simparity-wire-aws-v1) against the IVS emulator (service token `ivs`, crate services/service-com-amazonaws-ivs-amazoninteractivevideoservice) leaves ONE net-new divergence cluster that no open issue owns: the replay reports owned_by [] and stub_path "" for the CreateChannel cluster, while the four other IVS clusters in the same replay (GetChannel, UpdateChannel and the two BatchGetChannel playbackUrl clusters) DO carry owned_by and are skipped. This issue therefore has ONE cause, and the fixer must close it and re-replay its step before submitting.

Cause 1 of 1 — CreateChannel's 200 fills all three of a channel's announced address members with values a client of the emulator cannot dial: `$.channel.ingestEndpoint` and `$.channel.srt.endpoint` with a bare IP literal plus a port, and `$.channel.playbackUrl` with a minted hostname whose host this emulator does not serve. Fixture ivs-channel-lifecycle, step task#1; the cluster also covers ivs-viewer-session-revocation#setup#1.

AWS: recorded behavior only — the model states no form. Smithy (services/service-com-amazonaws-ivs-amazoninteractivevideoservice/smithy-model/com-amazonaws-ivs-amazoninteractivevideoservice.json, com.amazonaws.ivs, restJson1; smithy_query query="operation" operation="CreateChannel" and query="shape" shape="Channel,Srt,IngestEndpoint,PlaybackURL,SrtEndpoint"): CreateChannel POST /CreateChannel -> 200 {channel: Channel, streamKey: StreamKey}, errors AccessDeniedException 403 / PendingVerification 403 / ResourceNotFoundException 404 / ServiceQuotaExceededException 402 / ValidationException 400. `Channel$ingestEndpoint` -> `IngestEndpoint`, `Channel$playbackUrl` -> `PlaybackURL` and `Srt$endpoint` -> `SrtEndpoint` are plain strings with NO @length and NO @pattern; their docs are "Channel ingest endpoint, part of the definition of an ingest server, used when you set up streaming software", "Channel playback URL" and "The endpoint to be used when streaming with IVS using the SRT protocol". Because the model fixes no form, the recording is the authority, and the corpus does not pin these three values literally (it reports them `<shaped-not-asserted>`, since label, Region, account and channel id are per-run): it pins their CLASS. That class is (a) for `ingestEndpoint` and the `srt.endpoint` that equals it, a HOSTNAME carrying NO port — an ingest server a client dials by appending the protocol's own port, `rtmps://<ingestEndpoint>:443/app/<streamKey>` or `rtmp://<ingestEndpoint>:1935/...`; and (b) for `playbackUrl`, an absolute `https://` URL, laid out `/api/video/v1/<region>.<account>.channel.<channel-id>.m3u8`, that a player FETCHES exactly as it stands and therefore whose host must resolve, for a client of the service, to something that serves that path. The hostname form without a port, the shared ingest/SRT host, the scheme and the path layout are the requirement; the particular label, zone, account and channel id of any one run are not. No status or error code diverges here — the response is a correct 200.

Emulator: the replay reports verdict mismatch on an otherwise correct 200 (expected_status 200, actual_status 200) with three divergent keys: `$.channel.ingestEndpoint` value_mismatch (expected '<shaped-not-asserted>', got '127.0.0.1:36387'), `$.channel.srt.endpoint` value_mismatch (same value), and `$.channel.playbackUrl` value_mismatch (expected '<shaped-not-asserted>', got 'https://<12-hex>.<region>.playback.ivs.<zone>/api/video/v1/<region>.<account>.channel.<id>.m3u8'). So the two ingest members are now the raw authority of the socket the contribute plane bound — a loopback IP literal WITH a port, not a hostname — and the playback host is a minted name in a zone derived from the request's reached authority, which the sibling GetChannel/UpdateChannel/BatchGetChannel clusters in this same replay reject as host_not_own (expected and actual text identical; the host is not one of the emulator's own hosts).

Why: a client does not treat either member as an opaque string. Streaming software and the AWS-documented publish recipe compose `rtmps://<ingestEndpoint>:443/app/<stream key>`; handed '127.0.0.1:36387' it composes 'rtmps://127.0.0.1:36387:443/app/...', an authority with two ports that no encoder, URL parser or SRT client accepts, and a client that renders the value as "ingest server" in a UI or asserts TLS by hostname has no name to use at all — so ingest never starts and no stream goes LIVE even though the plane behind that socket would have answered. On the playback side the player GETs the URL verbatim; the announced host resolves to the emulator only for a client inside the emulator process, so any real client gets a name-resolution failure where AWS serves the HLS master playlist. Both halves of the IVS data plane are unusable while the control-plane response reports success.

Fix: the announced-address projection in services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/state/channel_endpoints.rs — `announced_channel`, which fills `ingest_endpoint` and the `srt` endpoint straight from the `contribute` authority it is passed, and `announced_playback_url`/`playback_host_in`/`announced_zone`, which compose the playback host — together with the planes they name (`super::contribute_authority`/`super::ensure_ingest_plane` and `super::playback_plane`/`super::publish_playback_addresses`). Both members must be announced as an AWS-shaped hostname with no port that an OUT-OF-PROCESS client can resolve to the plane that answers it; a hostname the emulator actually serves (resolvable for every client of the emulator, pointed at the bound plane) satisfies the class the recording pins, whereas either a bare socket authority or a name only the in-process register answers does not. Not the serializer and not the store: create_channel.rs persists none of the three members and delegates all of them to this projection.

Traces: ivs-channel-lifecycle#task#1 (representative), ivs-viewer-session-revocation#setup#1.

Related open issues, neither of which owns this cluster (the replay reports owned_by [] for it): 20261125T153458Z-ivs-5-net-new-causes-announced-channel-addresses.md carries CreateChannel replay keys for `$.channel.ingestEndpoint` and `$.channel.playbackUrl` from an earlier emulator state in which the ingest members were minted `*.global-contribute.ivs.<zone>` hostnames — the ingest members have since changed class to a bare IP:port, which that issue does not describe, and it is the owner of the four playbackUrl clusters this replay skips; 20261123T105030Z-ivs-2-net-new-causes-createchannel-announces-loopback.md covers the `$.channel.srt.endpoint` member's FORM only. Both are named here for context, not as coverage.

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, bootstrap_signal false, emulator_up true ("/__admin/health answered 200 and the :41825 data listener accepts"), fixtures_replayed 15, steps_total 84, steps_passed 66, steps_mismatched 13, steps_blocked 5, steps_errored 0, score_pct 78.6, divergence_count 5. Four of the five clusters carry a non-empty owned_by (all -> 20261125T153458Z-ivs-5-net-new-causes-announced-channel-addresses.md: BatchGetChannel#task#3, BatchGetChannel#task#4, GetChannel#task#2, UpdateChannel#task#4) and are skipped. The cluster below is net-new: owned_by [], stub_path "".

Cause 1 cluster (net-new):
  divergence_key: service=ivs|CreateChannel|status=200->200|keys=$.channel.ingestEndpoint,$.channel.playbackUrl,$.channel.srt.endpoint
  verdict mismatch; expected_status 200, actual_status 200 (the status is CORRECT; no error code is involved)
  fixture_id ivs-channel-lifecycle, step_key task#1; affected_count 2, other member ivs-viewer-session-revocation#setup#1
  detail:
    $.channel.ingestEndpoint: value_mismatch (expected '<shaped-not-asserted>', got '127.0.0.1:36387')
    $.channel.playbackUrl: value_mismatch (expected '<shaped-not-asserted>', got 'https://897b14e7ad41.us-west-2.playback.ivs.localhost/api/video/v1/us-west-2.000000000000.channel.9d81557a2b19.m3u8')
    $.channel.srt.endpoint: value_mismatch (expected '<shaped-not-asserted>', got '127.0.0.1:36387')

Per-run values vs requirement: the IP '127.0.0.1' and the port '36387', the hex label '897b14e7ad41', the zone 'localhost', the account '000000000000' and the channel id '9d81557a2b19' are ONE run's values and change every run — they are not the finding. What diverges is the CLASS of each member: a hostname with no port, equal for ingestEndpoint and srt.endpoint, resolvable by an out-of-process client to the ingest plane; and an https URL whose host an out-of-process client can fetch, carrying the /api/video/v1/<region>.<account>.channel.<id>.m3u8 path. The emulator serves a socket authority (IP + port) for the first two and an in-process-only name for the third — the same playback value the sibling clusters in this replay reject with verdict host_not_own, where expected and actual TEXT are byte-identical.

Smithy used: CreateChannel POST /CreateChannel, output {channel: Channel, streamKey: StreamKey}, errors AccessDeniedException 403 / PendingVerification 403 / ResourceNotFoundException 404 / ServiceQuotaExceededException 402 / ValidationException 400. Channel$ingestEndpoint -> IngestEndpoint (plain string, no @length, no @pattern), Channel$playbackUrl -> PlaybackURL (idem), Srt$endpoint -> SrtEndpoint (idem) — hence "recorded behavior only" for the form.

Source read for the fix location: src/state/channel_endpoints.rs — `announced_channel` (lines 138-158) sets `ingest_endpoint = contribute.map(str::to_string)` and copies it into the `srt` block, i.e. the member is the plane's `host:port` authority verbatim (the module's own RULE at lines 7-20 adopts that socket authority deliberately, including its port); `announced_playback_url` (lines 187-197) and `playback_host_in` (lines 217-226) compose `https://<minted-label>.<region>.playback.ivs.<zone><path>`; `announced_zone` (lines 270-272) derives that zone from the request's reached authority rather than from a listener this process bound, and the name is only registered for in-process clients (`publish_playback_addresses`, referenced lines 55-59).

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), or exercise the cause by hand against the data listener in region us-west-2:

1. POST /CreateChannel with a body naming only `name` (fixture ivs-channel-lifecycle, step task#1; ivs-viewer-session-revocation#setup#1 issues the same call in its setup phase). The response is 200.
2. Read `$.channel.ingestEndpoint`, `$.channel.srt.endpoint` and `$.channel.playbackUrl` from the response body.
3. Observe the two ingest members are the contribute plane's socket authority — an IP literal followed by ':<port>' — and not a hostname: AWS returns an ingest server as a hostname carrying NO port, which the client combines with the protocol's port itself.
4. Compose the publish URL the way a client does: `ffmpeg ... -f flv "rtmps://<ingestEndpoint>:443/app/<streamKey.value>"`. The authority now carries two ports and is rejected before any connection is attempted; the same value used as `srt://<srt.endpoint>:<port>` fails the same way.
5. From OUTSIDE the emulator process, resolve the host of `$.channel.playbackUrl` and GET the URL as it stands (`curl "<playbackUrl>"`). The host does not resolve: it is a minted label under `<region>.playback.ivs.<zone>` that only the emulator's in-process register answers. AWS serves the HLS master playlist there.
6. Re-run: the IP, port, hex labels, zone, account and channel id all change while the divergence persists — the per-run values are not the finding.

After a fix, re-replay ivs-channel-lifecycle#task#1 (and its cluster member ivs-viewer-session-revocation#setup#1).
