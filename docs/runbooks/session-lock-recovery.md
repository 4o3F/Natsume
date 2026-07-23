
# Session lock recovery

- A lock completes only after the Session Agent confirms its desktop gate for the exact session instance/epoch.
- Agent restart in the same session reasserts an active lock. A new session epoch invalidates the old lock/unlock target.
- Never replay an old unlock. Create a new unlock that matches `session_instance_id + session_epoch + lock_epoch + lock_command_id`.
- If logind signals succeed but Agent confirmation does not arrive, keep the desktop gate locked and repair/restart the Agent.
- Do not inspect, block, reload or repair Caddy as part of this runbook; Gateway state is independent.
