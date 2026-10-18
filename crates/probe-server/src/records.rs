//! The records service: CRUD on `records`, an entry in `record_events` for each
//! change (same transaction), and a broadcast of the change to every client.
//! All three transports call into this, so they behave the same.

use probe_protocol::{
    ChangeEvent, ConnectionType, RecordInput, RecordOp, RecordRow, Request, RpcData, RpcResult,
};
use sqlx::{PgPool, Postgres, Transaction};
use tokio::sync::broadcast;

use crate::error::AppError;

/// The list returns at most this many rows (newest first), so one response
/// always fits comfortably in an MQTT message.
pub const LIST_LIMIT: i64 = 500;

#[derive(Clone)]
pub struct Records {
    pool: PgPool,
    events: broadcast::Sender<ChangeEvent>,
}

impl Records {
    pub fn new(pool: PgPool) -> Self {
        // Slow receivers that fall more than 256 events behind get a `Lagged`
        // error and resync by listing again.
        let (events, _) = broadcast::channel(256);
        Self { pool, events }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ChangeEvent> {
        self.events.subscribe()
    }

    pub async fn list(&self) -> Result<Vec<RecordRow>, AppError> {
        let rows = sqlx::query_as!(
            RecordRow,
            r#"SELECT id, time, connection_type AS "connection_type: ConnectionType", value, updated_at
               FROM records ORDER BY time DESC, id DESC LIMIT $1"#,
            LIST_LIMIT
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn create(
        &self,
        via: ConnectionType,
        input: RecordInput,
    ) -> Result<RecordRow, AppError> {
        input.validate().map_err(AppError::BadRequest)?;
        let mut tx = self.pool.begin().await?;
        let record = sqlx::query_as!(
            RecordRow,
            r#"INSERT INTO records (connection_type, value) VALUES ($1, $2)
               RETURNING id, time, connection_type AS "connection_type: ConnectionType", value, updated_at"#,
            via as ConnectionType,
            input.value
        )
        .fetch_one(&mut *tx)
        .await?;
        log_event(
            &mut tx,
            via,
            RecordOp::Create,
            record.id,
            record.value.as_deref(),
        )
        .await?;
        tx.commit().await?;
        self.publish(ChangeEvent::Create {
            record: record.clone(),
        });
        Ok(record)
    }

    pub async fn update(
        &self,
        via: ConnectionType,
        id: i64,
        input: RecordInput,
    ) -> Result<RecordRow, AppError> {
        input.validate().map_err(AppError::BadRequest)?;
        let mut tx = self.pool.begin().await?;
        let record = sqlx::query_as!(
            RecordRow,
            r#"UPDATE records SET value = $2, connection_type = $3, updated_at = now()
               WHERE id = $1
               RETURNING id, time, connection_type AS "connection_type: ConnectionType", value, updated_at"#,
            id,
            input.value,
            via as ConnectionType
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound(id))?;
        log_event(&mut tx, via, RecordOp::Update, id, record.value.as_deref()).await?;
        tx.commit().await?;
        self.publish(ChangeEvent::Update {
            record: record.clone(),
        });
        Ok(record)
    }

    pub async fn delete(&self, via: ConnectionType, id: i64) -> Result<(), AppError> {
        let mut tx = self.pool.begin().await?;
        let deleted = sqlx::query!("DELETE FROM records WHERE id = $1 RETURNING value", id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(AppError::NotFound(id))?;
        log_event(&mut tx, via, RecordOp::Delete, id, deleted.value.as_deref()).await?;
        tx.commit().await?;
        self.publish(ChangeEvent::Delete {
            id,
            connection_type: via,
        });
        Ok(())
    }

    /// Runs one WSS/MQTT request and packs the outcome for the wire.
    pub async fn handle(&self, via: ConnectionType, request: Request) -> RpcResult {
        let result = match request {
            Request::List => self
                .list()
                .await
                .map(|records| RpcData::Records { records }),
            Request::Create { value } => self
                .create(via, RecordInput { value })
                .await
                .map(|record| RpcData::Record { record }),
            Request::Update { id, value } => self
                .update(via, id, RecordInput { value })
                .await
                .map(|record| RpcData::Record { record }),
            Request::Delete { id } => self.delete(via, id).await.map(|()| RpcData::Deleted { id }),
        };
        match result {
            Ok(data) => RpcResult::Ok { data },
            Err(e) => {
                e.log();
                RpcResult::Error { error: e.to_api() }
            }
        }
    }

    fn publish(&self, event: ChangeEvent) {
        // An error here only means nobody is listening right now.
        let _ = self.events.send(event);
    }
}

async fn log_event(
    tx: &mut Transaction<'_, Postgres>,
    via: ConnectionType,
    op: RecordOp,
    record_id: i64,
    value: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO record_events (connection_type, op, record_id, value) VALUES ($1, $2, $3, $4)",
        via as ConnectionType,
        op as RecordOp,
        record_id,
        value
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}
