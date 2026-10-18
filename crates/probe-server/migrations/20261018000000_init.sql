-- Initial schema for the probe.
-- sqlx runs each migration in a transaction and records it in _sqlx_migrations.

CREATE EXTENSION IF NOT EXISTS timescaledb;

CREATE TYPE connection_type AS ENUM ('https', 'wss', 'mqtt');
CREATE TYPE record_op AS ENUM ('create', 'update', 'delete');

-- The live table shown in the UI. A plain Postgres table: rows are updated and
-- deleted in place, which is what hypertables are not built for.
CREATE TABLE records (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    time            timestamptz NOT NULL DEFAULT now(),
    -- The transport that created the row or changed it last.
    connection_type connection_type NOT NULL,
    value           text,
    updated_at      timestamptz NOT NULL DEFAULT now()
);

-- Append-only log of every operation, whatever the transport. A hypertable:
-- TimescaleDB splits it into daily chunks, compresses chunks older than a day
-- and drops chunks older than seven days.
-- No foreign key to records: deleted records keep their history.
CREATE TABLE record_events (
    time            timestamptz NOT NULL DEFAULT now(),
    connection_type connection_type NOT NULL,
    op              record_op NOT NULL,
    record_id       bigint NOT NULL,
    value           text
);

SELECT create_hypertable('record_events', by_range('time', INTERVAL '1 day'));

CREATE INDEX record_events_record_id_time_idx ON record_events (record_id, time DESC);

ALTER TABLE record_events SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'connection_type',
    timescaledb.compress_orderby = 'time DESC'
);

SELECT add_compression_policy('record_events', INTERVAL '1 day');
SELECT add_retention_policy('record_events', INTERVAL '7 days');
