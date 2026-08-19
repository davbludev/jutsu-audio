# Tasks

`tasks/` is the only task tracker in this repository. The rules that matter every time also
live in `CLAUDE.md` under `## Tasks`.

## Layout

One directory per epic. The dotted, zero-padded `id` is also the filename prefix, so a plain
`ls` sorts the directory back into its tree and every parent groups its own children.

```
tasks/
  01-identity-and-access/
    _epic.md                                     id 01
    01.02-customer-email-verification.md         id 01.02      feature
    01.02.01-fix-verification-suppression.md     id 01.02.01   bug under that feature
  bugs/    B01-…   bugs with no epic; where new bug reports go
  misc/    M01-…   chores and features with no epic
```

A bug that belongs to a feature stays with that feature, so an epic directory always shows
all of its own work. `tasks/bugs/` is only for bugs that have no epic home.

IDs never change. Renaming a slug is fine; renumbering is not.

Filenames are `<id>-<slug>.md`, slug lowercase and hyphenated, cut at a word boundary around
50 characters. The full wording lives in `title:`.

## Frontmatter

```yaml
id: 06.04              # never changes
title: …
status: todo | doing | done
type: epic | feature | task | bug | chore | research
priority: low | medium | high | critical
depends: [03.02]       # omit when empty
docs: [docs/plans/06.04.md]     # omit when empty
started: 2026-08-14    # YYYY-MM-DD, empty until work begins
completed:
```

`status:` is the only place status is recorded. Never write it a second time anywhere — not
as a checkbox in a parent, not in a summary table. A second copy drifts, and then neither
copy can be trusted. For the same reason there is no generated index: `grep` rebuilds any
view on demand.

## Finding work

```bash
grep -rl "^status: doing" tasks/     # everything in progress
grep -rl "^status: todo" tasks/      # the backlog
grep -rl "^type: bug" tasks/         # every bug, wherever it lives
ls tasks/*/06.04*                    # the file for a known id
```

## Working with a task

- Change status by editing the one `status:` line. Moving to `doing` fills `started:`;
  moving to `done` fills `completed:`. Never rewrite a whole task file.
- Record progress by appending one dated bullet under `## Notes`. Never rewrite `## Notes` —
  it is the audit trail.
- New task: copy the frontmatter block, take the next free number under its parent. Numbers
  are never reused, even after a delete.
- A task that grows into several tasks gets children (`06.04.01`, `06.04.02`) rather than a
  renumbering.
