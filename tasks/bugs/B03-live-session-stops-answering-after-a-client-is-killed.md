---
id: B03
title: A live session stops answering after a CLI client is killed mid-request
status: todo
type: bug
priority: medium
reported: 2026-08-19
---

## What happens

After a `jutsu-audio-cli` process is terminated while it is waiting on the session socket, the
running editor keeps its `.session` sidecar in place but no longer answers session requests:

- `session_status` reports `attached: false` even though the editor process is alive and its
  window is responsive.
- `batch` still refuses with `session_unavailable` — "an editor has this project open" — because
  that check looks at the sidecar file rather than at whether the endpoint answers.

The project is therefore neither editable through the session nor editable offline. The only way
out is to close the editor, delete the sidecar and reopen.

## How it was hit

2026-08-19, working live on `%USERPROFILE%\Documents\Jutsu Audio\lab.jutsu-audio.json`:

1. Two `transport_request` calls sent seconds after the editor launched blocked on the socket
   (the project was still loading) and were killed with `Stop-Process`.
2. `session_status` answered normally for a while afterwards, then began reporting
   `attached: false` while the editor kept running.
3. `batch` refused with `session_unavailable`; `inspect_project` fell through to the offline
   route and succeeded, which is how the mismatch became visible.

## Why it matters

The two checks disagree about whether a session exists, and a killed client — an ordinary thing
during agent work, a timeout, or a closed terminal — is enough to make them disagree. The
discovery code already handles the case of a session file whose owner is gone (nothing answers
its port, so the file is cleaned up); this is the same situation with the owner still alive but
its accept loop no longer serving.

## Where to look

- `crates/jutsu-audio-session/src/discovery.rs` — sidecar publication and the
  nothing-answers-its-port cleanup path.
- `crates/jutsu-audio-session/src/client.rs` — `SessionClient::attach`, and what a client leaves
  behind when it dies mid-request.
- `src/session_host.rs` — `SessionHost::start` and `poll`; the server's accept loop is the thing
  to check for a connection that is never reaped.
- The `session_unavailable` refusal in the batch path (`src/cli_batch.rs` / `src/cli_session.rs`)
  decides on the sidecar rather than on a live probe.

## Notes

- 2026-08-19 — filed while building the first cue with the `jutsu-audio` skill. A guard worth
  considering separately: requests sent while a project is still loading block rather than
  failing fast, which is what led to the kill in the first place.
