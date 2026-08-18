use diesel::{
    QueryableByName, RunQueryDsl,
    sql_types::{Nullable, Text},
};
use natsume_integration_tests::harness::{TestServer, require_ok};
use serde_json::Value;
use tokio_tungstenite::tungstenite::{
    Error as WebSocketError, handshake::client::Response as WebSocketResponse,
};
use uuid::Uuid;

use super::wire::{self, TestWebSocket};

#[derive(QueryableByName)]
struct AuditDetailRow {
    #[diesel(sql_type = Text)]
    redacted_detail_json: String,
}

#[derive(QueryableByName)]
pub(super) struct TerminalAuditRow {
    #[diesel(sql_type = Text)]
    pub(super) actor: String,
    #[diesel(sql_type = Text)]
    pub(super) action_kind: String,
    #[diesel(sql_type = Text)]
    pub(super) resource_type: String,
    #[diesel(sql_type = Nullable<Text>)]
    pub(super) resource_id: Option<String>,
    #[diesel(sql_type = Text)]
    pub(super) result: String,
    #[diesel(sql_type = Nullable<Text>)]
    pub(super) reason_code: Option<String>,
    #[diesel(sql_type = Text)]
    pub(super) correlation_id: String,
    #[diesel(sql_type = Text)]
    pub(super) redacted_detail_json: String,
}

pub(super) trait WssServerFixture {
    async fn websocket_attempt(
        &self,
        token: Option<&str>,
        subprotocol: Option<&str>,
        cookie: Option<&str>,
    ) -> Result<(TestWebSocket, WebSocketResponse), WebSocketError>;

    fn terminal_audits(&self, command_id: Uuid) -> Vec<TerminalAuditRow>;

    fn issuance_eviction_flags(&self) -> Vec<bool>;
}

impl WssServerFixture for TestServer {
    async fn websocket_attempt(
        &self,
        token: Option<&str>,
        subprotocol: Option<&str>,
        cookie: Option<&str>,
    ) -> Result<(TestWebSocket, WebSocketResponse), WebSocketError> {
        wire::websocket_attempt(self, token, subprotocol, cookie).await
    }

    fn terminal_audits(&self, command_id: Uuid) -> Vec<TerminalAuditRow> {
        let mut connection = self.observer();
        require_ok(
            diesel::sql_query(
                "SELECT actor, action_kind, resource_type, resource_id, result, reason_code, \
                 correlation_id, redacted_detail_json FROM audit_events \
                 WHERE action_kind = 'command_terminal' AND resource_id = ? ORDER BY rowid",
            )
            .bind::<Text, _>(command_id.to_string())
            .load::<TerminalAuditRow>(&mut connection),
            "terminal Command audits must be readable",
        )
    }

    fn issuance_eviction_flags(&self) -> Vec<bool> {
        let mut connection = self.observer();
        let rows = require_ok(
            diesel::sql_query(
                "SELECT redacted_detail_json FROM audit_events \
                 WHERE action_kind = 'issue_device_credentials' ORDER BY rowid",
            )
            .load::<AuditDetailRow>(&mut connection),
            "issuance audit rows must be readable",
        );
        rows.into_iter()
            .map(|row| {
                let detail: Value = require_ok(
                    serde_json::from_str(&row.redacted_detail_json),
                    "issuance audit detail must be JSON",
                );
                detail
                    .get("evicted_live_connection")
                    .and_then(Value::as_bool)
                    .unwrap_or_else(|| panic!("issuance eviction evidence must be boolean"))
            })
            .collect()
    }
}
