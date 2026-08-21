-- Server-side settings the mobile app edits.
--
-- Until now every user-facing setting lived on the phone (AsyncStorage /
-- SecureStore), which works while the setting only affects the phone. The
-- processing schedule does not: the *worker* has to obey it, it must outlive
-- any single device, and two phones must not disagree about it. So it lives
-- here, as one JSONB document per key.
--
-- Deliberately a key/value table rather than a column per setting: these are
-- documents the API validates, not things anything joins or indexes on, and a
-- new one should not cost a migration.
CREATE TABLE app_settings (
    key        text        PRIMARY KEY,
    value      jsonb       NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);

-- No seed row. An absent key means "defaults", and the default schedule is
-- disabled — so an existing install keeps processing exactly as it does today
-- until someone turns the schedule on from the app.
