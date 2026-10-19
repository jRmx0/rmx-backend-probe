-- Login with server-side sessions.
--
-- users: one row per person who may log in. There is no sign-up page; users
-- are created with `probe-server create-user <name>` (README, "Users").
-- sessions: one row per logged-in browser. The cookie holds a random 256-bit
-- token; only its SHA-256 is stored here, so someone who can read this table
-- (a leaked dump, a backup) still can't use it to log in.

CREATE TABLE users (
    id                  bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    -- Stored lower-case; logins are case-insensitive.
    username            text NOT NULL UNIQUE
                        CHECK (username ~ '^[a-z0-9][a-z0-9._-]{0,31}$'),
    -- PHC string, e.g. $argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>.
    -- It carries its own parameters, so stronger settings later only affect new hashes.
    password_hash       text NOT NULL,
    created_at          timestamptz NOT NULL DEFAULT now(),
    password_changed_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE sessions (
    -- SHA-256 of the cookie value.
    id_hash      bytea PRIMARY KEY CHECK (length(id_hash) = 32),
    -- Deleting a user logs them out everywhere.
    user_id      bigint NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    -- Sliding (idle) timeout: the session ends when it has not been used for a while.
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    -- Absolute timeout: the session ends at this time however active it is.
    expires_at   timestamptz NOT NULL,
    -- Only to tell sessions apart ("which phone is this?"), at most 200 characters.
    user_agent   text
);

CREATE INDEX sessions_user_id_idx ON sessions (user_id);
CREATE INDEX sessions_expires_at_idx ON sessions (expires_at);
