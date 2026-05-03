# Data preprocessing

This module converts the contest team XLSX sheet into DOMjudge-compatible import files and can upload organization logos after organizations are created.

## Input XLSX schema

Both scripts expect the first sheet to use this exact header order:

```text
organization,team_name_en,team_name_zh,seat,account,password,category
```

Column usage:

| Column | Usage |
| --- | --- |
| `organization` | Deduplicated into `organizations.json` and mapped to generated `INST-*` IDs. |
| `team_name_en` | Combined into the generated team `name` and `display_name`. |
| `team_name_zh` | Combined into the generated team `name` and `display_name`. |
| `seat` | Written to `team.location.description` when non-empty. |
| `account` | Used as team ID, ICPC ID, label, account username, and account team ID. |
| `password` | Written to `accounts.yaml`. |
| `category` | Written as the generated team's only `group_ids` entry. |

The main converter reads the XLSX sheet as strings with `fastexcel`, preserving account/password/seat values and common Excel error literals such as `#N/A` and `#VALUE!` instead of dropping them during conversion.

## Generate DOMjudge files

Install dependencies with `uv`, then run:

```bash
uv run python data_preprocess.py <teams.xlsx>
```

The script writes these files in the current working directory:

- `organizations.json`
- `teams.json`
- `accounts.yaml`

Generated teams use `team_name_zh(team_name_en)` for both `name` and `display_name`.

## Upload organization logos

After organizations exist in DOMjudge, place logo image files in one folder. The script matches each organization to a logo by normalizing the organization name and filename: it removes the extension, lowercases the name, and strips non-word characters.

Run:

```bash
uv run python upload_organization_logo.py <teams.xlsx> <logo-folder> <api-domain>
```

The script prompts for API username/password, builds a Basic Auth token, resizes each matched logo to `64x64`, converts it to PNG, and uploads it to:

```text
<api-domain>/api/v4/organizations/<org-id>/logo
```

Organizations without a matching logo file are reported at the end.
