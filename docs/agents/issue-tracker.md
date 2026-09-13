# Issue tracker: GitHub

Issues and specs for this repo live in GitHub Issues for `4o3F/Natsume`.
Use the `gh` CLI from this clone; outside it, pass `--repo 4o3F/Natsume`.

## Conventions

- Create an issue: `gh issue create --title "..." --body-file <file>`.
- Read an issue: `gh issue view <number> --comments`. Fetch structured
  details with `--json number,title,body,labels,comments`.
- List issues: `gh issue list --state open --json number,title,body,labels,comments`,
  adding appropriate `--label` and `--state` filters.
- Comment: `gh issue comment <number> --body-file <file>`.
- Apply or remove labels: `gh issue edit <number> --add-label "..."` or
  `gh issue edit <number> --remove-label "..."`.
- Close: `gh issue close <number>`.

Write multiline issue bodies and comments to a temporary UTF-8 file and
pass it with `--body-file`, preserving actual newlines.

## Pull requests as a triage surface

**PRs as a request surface: no.**

## When a skill says "publish to the issue tracker"

Create a GitHub issue.

## When a skill says "fetch the relevant ticket"

Run `gh issue view <number> --comments`.

## Wayfinding operations

Used by `/wayfinder`. The map is one issue with child issues as tickets.

- Map: an issue labelled `wayfinder:map`, containing Notes,
  Decisions-so-far, and Fog.
- Child ticket: link it as a GitHub sub-issue. If unavailable, add it to
  a task list in the map and put `Part of #<map>` in the child body.
  Use `wayfinder:<type>` labels for research, prototype, grilling, or task.
- Blocking: use native issue dependencies. Add an edge with
  `gh api --method POST repos/4o3F/Natsume/issues/<child>/dependencies/blocked_by -F issue_id=<blocker-db-id>`.
  Obtain the database ID with
  `gh api repos/4o3F/Natsume/issues/<blocker> --jq .id`.
- If native dependencies are unavailable, record
  `Blocked by: #<number>, #<number>` near the top of the child body.
  All blockers must be closed before the ticket is unblocked.
- Frontier: select the first open, unassigned, unblocked child in map order.
  For native dependencies, `issue_dependencies_summary.blocked_by` counts
  open blockers.
- Claim: `gh issue edit <number> --add-assignee @me`.
- Resolve: comment with the answer, close the child, and append a concise
  finding with a link to the map's Decisions-so-far.
