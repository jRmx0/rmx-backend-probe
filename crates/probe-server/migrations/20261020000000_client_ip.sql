-- The client IP behind each change.
--
-- records.created_ip: who created the row (kept when the row is updated).
-- record_events.ip: who made each create/update/delete.
-- NULL when the server doesn't know it: MQTT requests come through the
-- broker, which doesn't pass on the client's address.
--
-- IP addresses are personal data. They are shown only to logged-in users,
-- and record_events drops them with its chunks after 7 days.

ALTER TABLE records ADD COLUMN created_ip inet;

-- Allowed on a compressed hypertable: a nullable column without a default.
ALTER TABLE record_events ADD COLUMN ip inet;
