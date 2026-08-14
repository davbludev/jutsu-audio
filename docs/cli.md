# Jutsu Audio structured CLI

`jutsu-audio-cli` reads exactly one JSON request from standard input and writes exactly one JSON response to standard output. Protocol version `1` is stable for the MVP. Every entity result returns explicit UUIDs; agents never need to parse human prose.

Operations: `create_project`, `inspect_project`, `import_sample`, `add_clip`, `update_clip`, `delete_clip`, `export_wav`, and `transport_request`. Requests use snake_case tagged JSON with `protocol_version: 1`. Inspect output provides the complete project and the default `track_id`/`layer_id` needed for clip commands. `export_wav` accepts `encoding` (`pcm16` or `float32`) plus optional `start_frame` and `frame_count`.

Exit codes:

- `0`: structured success.
- `2`: malformed request or unsupported protocol version.
- `3`: project file, asset, or WAV failure.
- `4`: shared command validation or entity failure.

All exits, including failures, return an envelope with `ok`, `protocol_version`, and either `result` or `error { code, message }`. `transport_request` is acknowledged offline in the MVP; live GUI delivery belongs to the later collaboration phase.
