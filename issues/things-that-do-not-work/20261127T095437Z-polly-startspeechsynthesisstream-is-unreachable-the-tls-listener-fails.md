Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: polly.stream-idle-input-ends-with-an-inbound-event-timeout-error polly.stream-malformed-ssml-text-event-rejected polly.stream-synthesis-delivers-audio-events-in-the-requested-format polly.stream-text-event-after-close-stream-event-rejected
Issue-Shape-Id: com.amazonaws.polly#Parrot_v1
Issue-Key: polly.stream-idle-input-ends-with-an-inbound-event-timeout-error|_stream#7|AwsCrtError|url.hostname, port, tls_connection_options=tls).result(30)
Issue-Key: polly.stream-malformed-ssml-text-event-rejected|_stream#6|AwsCrtError|url.hostname, port, tls_connection_options=tls).result(30)
Issue-Key: polly.stream-synthesis-delivers-audio-events-in-the-requested-format|_stream#4|AwsCrtError|url.hostname, port, tls_connection_options=tls).result(30)
Issue-Key: polly.stream-text-event-after-close-stream-event-rejected|_stream#5|AwsCrtError|url.hostname, port, tls_connection_options=tls).result(30)
Issue-Commit: daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7

# Polly StartSpeechSynthesisStream is unreachable: the TLS listener fails ALPN/TLS negotiation for HTTP/2 clients, so every bidirectional synthesis stream dies in the handshake

> _Note: this issue was identified against the code base at commit `daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSpeechSynthesisStream

## Description
1 cause.

Cause 1 of 1: the emulator's TLS endpoint does not complete a handshake with an HTTP/2 client that requires ALPN protocol "h2", so POST /v1/synthesisStream can never be opened and the whole bidirectional streaming operation is unreachable.
AWS: Smithy binds StartSpeechSynthesisStream to POST /v1/synthesisStream with success status 200 and errors ServiceQuotaExceededException / ThrottlingException / ValidationException; its input and output are event streams ("Synthesizes UTF-8 input, plain text, or SSML over a bidirectional streaming connection"). A bidirectional event stream is only transportable over HTTP/2, so AWS's endpoint negotiates TLS with ALPN "h2" and then carries the request's event-stream frames and the response's AudioEvent / StreamClosedEvent frames on that one stream. Recorded behavior: the four checks each open an HTTP/2 connection to the service endpoint with ALPN list ["h2"], the server name set to the endpoint host and the run's own CA as the trust store, send SigV4 chain-signed TextEvent/CloseStreamEvent frames, and read a 200 with content-type application/vnd.amazon.eventstream. No status/error code is involved in this divergence: the connection never reaches HTTP.
Emulator: the handshake fails before any request is sent — awscrt.exceptions.AwsCrtError: AWS_IO_TLS_ERROR_NEGOTIATION_FAILURE: TLS (SSL) negotiation failed, raised from Http2ClientConnection.new(host, port, tls_connection_options=...).result(30) in all four stream checks. In the same run, 54 other polly/s3/cloudformation checks reach the SAME endpoint over HTTPS successfully through botocore with the same trust material, and the stream checks differ only in requiring HTTP/2 via ALPN — so the listener answers HTTP/1.1 TLS but not a client that insists on "h2". Hypothesis: the data-plane TLS listener advertises no ALPN protocol (or only http/1.1), so a client whose ALPN list is exactly ["h2"] gets a negotiation failure. The handler itself exists (services/service-com-amazonaws-polly-parrot-v1/src/ops/start_speech_synthesis_stream.rs) and the operation is routable, so nothing downstream of the transport was exercised.
Why: StartSpeechSynthesisStream is the only way to get streamed synthesis, and every SDK opens it over HTTP/2. A client gets a transport-level TLS failure instead of a stream, so none of the operation's modelled behavior is usable: no AudioEvent, no StreamClosedEvent, and none of its refusals (the inbound-event-timeout ServiceQuotaExceededException, the malformed-SSML ValidationException event, the refusal of a TextEvent after CloseStreamEvent) can ever be observed.
Fix: the emulator's TLS/HTTP-2 transport for the data-plane listener, not this operation's handler: the listener must offer ALPN "h2" (while keeping http/1.1 for the SDK clients that use it) and serve the event-stream request/response over that connection. Once the handshake lands, re-replay the four checks below — their assertions on the event sequence and on the error events are unverified until then.
Traces: polly.stream-synthesis-delivers-audio-events-in-the-requested-format, polly.stream-idle-input-ends-with-an-inbound-event-timeout-error, polly.stream-malformed-ssml-text-event-rejected, polly.stream-text-event-after-close-stream-event-rejected.

Note on attribution: this could in principle be the replay client's own TLS stack rather than the emulator's listener. The run's evidence points at the listener: the same process, the same CA material and the same endpoint work for 54 HTTPS checks via botocore, and awscrt itself loads and runs fine in-process (the same module is imported successfully by polly.stream-closed-event-counts-the-characters-of-every-text-event, which failed later on an unrelated assertion). It is filed because an HTTP/2-only operation that cannot be connected to at all is a complete loss of that operation.

## Evidence
Behavioral suite replay against this checkout: 54/68 passed (79.4%), bootstrap_signal=false. Four polly checks failed identically at connection setup:

  [error] polly.stream-idle-input-ends-with-an-inbound-event-timeout-error
  [error] polly.stream-malformed-ssml-text-event-rejected
  [error] polly.stream-synthesis-delivers-audio-events-in-the-requested-format
  [error] polly.stream-text-event-after-close-stream-event-rejected

Identical tail for all four:
  File "_base.py", line 401, in __get_result
    raise self._exception
  awscrt.exceptions.AwsCrtError: AWS_IO_TLS_ERROR_NEGOTIATION_FAILURE: TLS (SSL) negotiation failed

The failing call, from the checks' shared _stream() helper:
  options = crt_io.TlsContextOptions()
  if isinstance(ctx.http_verify, str):
      options.override_default_trust_store_from_path(ca_filepath=ctx.http_verify)
  tls = crt_io.ClientTlsContext(options).new_connection_options()
  tls.set_server_name(url.hostname)
  tls.set_alpn_list(["h2"])
  conn = crt_http.Http2ClientConnection.new(url.hostname, port, tls_connection_options=tls).result(30)

Expected (recording): the connection is established, POST /v1/synthesisStream returns HTTP 200 with content-type application/vnd.amazon.eventstream, and the decoded frames are AudioEvent... then exactly one StreamClosedEvent last (or, for the negative checks, a single ServiceQuotaExceededException / ValidationException event).
Actual: no HTTP exchange at all — the TLS handshake fails, so status is None and no events are decoded.

Control: in the same run, 54 checks (including polly DescribeVoices/SynthesizeSpeech/StartSpeechSynthesisTask and s3) reach the same endpoint over HTTPS via botocore with the same ctx.http_verify trust material and pass; awscrt itself is importable and usable in the runner process.

Model facts used: smithy_query(query="digest") on services/service-com-amazonaws-polly-parrot-v1/smithy-model/com-amazonaws-polly-parrot-v1.json — StartSpeechSynthesisStream: POST /v1/synthesisStream, input StartSpeechSynthesisStreamInput, output StartSpeechSynthesisStreamOutput (event streams), errors [ServiceQuotaExceededException, ThrottlingException, ValidationException]. Handler present at services/service-com-amazonaws-polly-parrot-v1/src/ops/start_speech_synthesis_stream.rs.

## Steps to Reproduce
1. Boot the emulator from this checkout and note its HTTPS data-plane endpoint and the CA file the run trusts.
2. From Python with awscrt installed, build a TLS context whose trust store is that CA file, set the server name to the endpoint host and set the ALPN list to exactly ["h2"], then call awscrt.http.Http2ClientConnection.new(host, port, tls_connection_options=tls).result(30).
3. Observe AwsCrtError AWS_IO_TLS_ERROR_NEGOTIATION_FAILURE instead of an established connection. For comparison, a botocore client against the same endpoint with the same CA file succeeds (HTTP/1.1), which isolates the failure to the HTTP/2 ALPN negotiation.
4. Once the connection can be opened, POST /v1/synthesisStream with SigV4 on the request plus a chained chunk signature on each event-stream frame (TextEvent, then CloseStreamEvent, then the empty end frame) and confirm a 200 with content-type application/vnd.amazon.eventstream carrying AudioEvent frames followed by exactly one StreamClosedEvent.
5. Re-replay polly.stream-synthesis-delivers-audio-events-in-the-requested-format, polly.stream-idle-input-ends-with-an-inbound-event-timeout-error, polly.stream-malformed-ssml-text-event-rejected and polly.stream-text-event-after-close-stream-event-rejected.
