# DOMjudge roster export

This ZIP contains the entire committed Natsume roster and current passwords. Keep
it private, transfer it securely, and delete extracted credentials after use.
Candidate uploads and device bindings do not affect this export.

## Import into DOMjudge 9.0.1 or later

Use the administrator **Import / export** page, in this order:

1. `groups.json` (categories)
2. `organizations.json` (affiliations)
3. `teams.json` (teams)
4. `accounts.yaml` (team login accounts)

Alternatively, from DOMjudge's `webapp` directory, use its API command with
credentials configured according to your DOMjudge installation:

```sh
bin/console api:call -m POST -f json=groups.json users/groups
bin/console api:call -m POST -f json=organizations.json users/organizations
bin/console api:call -m POST -f json=teams.json users/teams
bin/console api:call -m POST -f yaml=accounts.yaml users/accounts
```

The import `id` values are external identifiers, not DOMjudge internal database
IDs. Category IDs come from the workbook. Organization IDs are Natsume's stable
INST IDs. Team/account IDs and usernames are the fixed workbook account. No
invented ICPC IDs are supplied. Bilingual team names use `中文名(English name)`.
Re-import all four files to apply a complete update; changing a Natsume password
alone does not update DOMjudge. Verify team login before releasing the contest.

Copy the contents of `logos/` into `<DOMSERVER_ROOT>/webapp/public/images/affiliations/`
(the directory used by DOMjudge's public assets), retaining each `INST-xxx.png`
filename. Give the web server read permission. Each image is actual PNG content,
not a renamed WebP. Raster dimensions and transparency are preserved; SVG uses
its natural 96 DPI canvas and transparent background. No source file is modified.

Missing or ambiguous logos are listed below and omitted; other schools' images
and all roster files are included. Add or rename images in Natsume's configured
source directory and export again. Source files are read during export and are
independent of the consistent database snapshot.

Reference: https://www.domjudge.org/docs/manual/9.0/import.html

## School and source image mapping
