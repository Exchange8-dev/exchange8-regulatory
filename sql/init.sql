-- SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
-- SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
-- SPDX-FileCopyrightText: Copyright Contributors to the Exchange8:Regulatory
-- SPDX-License-Identifier: Apache-2.0

-- Ex8:Regulatory — reporting sessions and report history.
--
-- Design: design/sentin8-exchange8-data-model-technical-design.md §3.8 (RG).
-- No migration code (new platform): `sentin8-cli vertical init` applies this.

CREATE SCHEMA IF NOT EXISTS regulatory;

-- Regime subscriber links (legacy FIXROC sessions generalized).
CREATE TABLE IF NOT EXISTS regulatory.regulatory_session (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    jurisdiction TEXT NOT NULL CHECK (jurisdiction IN
                ('iiroc','cat','mifir','fca','emir','dodd_frank')),
    format      TEXT NOT NULL CHECK (format IN ('fix','api','file','sftp')),
    transport   TEXT NOT NULL,
    address     TEXT,
    port        INTEGER,
    creds_secret_ref TEXT,               -- Sentin8:Secrets reference — never plaintext
    enabled     BOOLEAN NOT NULL DEFAULT TRUE,
    UNIQUE (env_id, jurisdiction)
);

-- Report history + ack state.
CREATE TABLE IF NOT EXISTS regulatory.regulatory_report (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    jurisdiction TEXT NOT NULL CHECK (jurisdiction IN
                ('iiroc','cat','mifir','fca','emir','dodd_frank')),
    seq         BIGINT NOT NULL,
    report_class TEXT NOT NULL CHECK (report_class IN
                ('new','modify','cancel','trade','quote')),
    payload     JSONB NOT NULL,
    ack_state   TEXT NOT NULL DEFAULT 'sent' CHECK (ack_state IN
                ('sent','acked','rejected','retrying','failed')),
    retry_count INTEGER NOT NULL DEFAULT 0,
    ts          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (env_id, jurisdiction, seq)
);
CREATE INDEX IF NOT EXISTS idx_exchange8_regreport_env_state ON regulatory.regulatory_report (env_id, ack_state);

-- ---------------------------------------------------------------------------
-- RLS (architecture-principles.md §4.3): enable + force on all tenant tables.
-- Policies use the platform helpers sentin8_context_initialized() and
-- sentin8_current_env_id(); services call set_tenant_context on the same
-- connection as the data query.
-- ---------------------------------------------------------------------------
DO $$
DECLARE tbl TEXT;
BEGIN
    FOREACH tbl IN ARRAY ARRAY['regulatory.regulatory_session', 'regulatory.regulatory_report'] LOOP
        BEGIN
            EXECUTE format('ALTER TABLE %s ENABLE ROW LEVEL SECURITY', tbl);
            EXECUTE format('ALTER TABLE %s FORCE ROW LEVEL SECURITY', tbl);
            EXECUTE format('DROP POLICY IF EXISTS %1$I_select ON %2$s;
CREATE POLICY %1$I_select ON %2$s FOR SELECT USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())', split_part(tbl, '.', 2), tbl);
            EXECUTE format('DROP POLICY IF EXISTS %1$I_insert ON %2$s;
CREATE POLICY %1$I_insert ON %2$s FOR INSERT WITH CHECK (sentin8_context_initialized() AND env_id = sentin8_current_env_id())', split_part(tbl, '.', 2), tbl);
            EXECUTE format('DROP POLICY IF EXISTS %1$I_update ON %2$s;
CREATE POLICY %1$I_update ON %2$s FOR UPDATE USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())', split_part(tbl, '.', 2), tbl);
            EXECUTE format('DROP POLICY IF EXISTS %1$I_delete ON %2$s;
CREATE POLICY %1$I_delete ON %2$s FOR DELETE USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())', split_part(tbl, '.', 2), tbl);
        EXCEPTION WHEN OTHERS THEN
            RAISE NOTICE 'RLS setup failed for %: %', tbl, SQLERRM;
        END;
    END LOOP;
END $$;

-- ---------------------------------------------------------------------------
-- Per-service role grants (schema-per-service: this service owns schema
-- `regulatory`; grants live in db `exchange8` only — design doc §1).
-- ---------------------------------------------------------------------------
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'exchange8_regulatory_service') THEN
        ALTER ROLE exchange8_regulatory_service SET search_path = regulatory, public;
        GRANT USAGE ON SCHEMA regulatory TO exchange8_regulatory_service;
        GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA regulatory TO exchange8_regulatory_service;
        GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA regulatory TO exchange8_regulatory_service;
    END IF;
END $$;

-- ---------------------------------------------------------------------------
-- Local outbox (DBX §6.7 — generated from `sentin8_outbox::outbox_table_sql(
-- "regulatory")`): rows written in the same DB transaction as the domain change;
-- the shell's relay (boot step §8b) pushes them to Bus `POST /v1/produce`.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS regulatory.outbox (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id          UUID NOT NULL,
    event_type      TEXT NOT NULL,
    stream_family   TEXT NOT NULL,
    payload         JSONB NOT NULL,
    publisher_id_token  TEXT,
    publisher_subject   TEXT,
    relayed         BOOLEAN NOT NULL DEFAULT false,
    relayed_at      TIMESTAMPTZ,
    relay_attempts  INT NOT NULL DEFAULT 0,
    next_retry_at   TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_regulatory_outbox_unrelayed
    ON regulatory.outbox (created_at) WHERE relayed = false;
DO $$ BEGIN
    EXECUTE 'ALTER TABLE regulatory.outbox ENABLE ROW LEVEL SECURITY';
    EXECUTE 'ALTER TABLE regulatory.outbox FORCE ROW LEVEL SECURITY';
    EXECUTE 'DROP POLICY IF EXISTS outbox_select ON regulatory.outbox';
    EXECUTE 'CREATE POLICY outbox_select ON regulatory.outbox FOR SELECT
        USING (sentin8_context_initialized()
               AND env_id = sentin8_current_env_id())';
    EXECUTE 'DROP POLICY IF EXISTS outbox_insert ON regulatory.outbox';
    EXECUTE 'CREATE POLICY outbox_insert ON regulatory.outbox FOR INSERT
        WITH CHECK (sentin8_context_initialized()
               AND env_id = sentin8_current_env_id())';
    EXECUTE 'DROP POLICY IF EXISTS outbox_update ON regulatory.outbox';
    EXECUTE 'CREATE POLICY outbox_update ON regulatory.outbox FOR UPDATE
        USING (sentin8_context_initialized()
               AND env_id = sentin8_current_env_id())';
EXCEPTION WHEN OTHERS THEN
    RAISE NOTICE 'RLS setup failed for regulatory.outbox: %', SQLERRM;
END $$;

CREATE OR REPLACE FUNCTION regulatory.regulatory_notify_outbox() RETURNS TRIGGER
LANGUAGE plpgsql AS $fn$
BEGIN
    PERFORM pg_notify('regulatory_outbox_changed', '');
    RETURN NEW;
END;
$fn$;
DROP TRIGGER IF EXISTS outbox_notify_trigger ON regulatory.outbox;
CREATE TRIGGER outbox_notify_trigger
    AFTER INSERT ON regulatory.outbox
    FOR EACH ROW
    EXECUTE FUNCTION regulatory.regulatory_notify_outbox();

-- ---------------------------------------------------------------------------
-- Local audit journal (DBX §6.8 — generated from
-- `sentin8_audit::audit_journal_sql("regulatory")`): rows written in the same
-- transaction as the domain change; the shell's pusher (boot step §8c) pushes
-- them to Sys `POST /v1/audit` (`sentin8:sys:audit:write`). A Sys outage
-- never drops audit.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS regulatory.audit_journal (
    id              UUID PRIMARY KEY,
    env_id          UUID NOT NULL,
    action          TEXT NOT NULL,
    target_resource_type TEXT,
    target_resource_id    TEXT,
    actor_user_id   TEXT,
    actor_subject_type TEXT,
    severity        TEXT NOT NULL DEFAULT 'info',
    correlation_id  TEXT,
    before_state    JSONB,
    after_state     JSONB,
    details         JSONB,
    pushed          BOOLEAN NOT NULL DEFAULT false,
    pushed_at       TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_regulatory_audit_journal_unpushed
    ON regulatory.audit_journal (created_at) WHERE pushed = false;
DO $$ BEGIN
    EXECUTE 'ALTER TABLE regulatory.audit_journal ENABLE ROW LEVEL SECURITY';
    EXECUTE 'ALTER TABLE regulatory.audit_journal FORCE ROW LEVEL SECURITY';
    EXECUTE 'DROP POLICY IF EXISTS audit_journal_dml ON regulatory.audit_journal';
    EXECUTE 'CREATE POLICY audit_journal_dml ON regulatory.audit_journal
        USING (sentin8_context_initialized()
               AND env_id = sentin8_current_env_id())
        WITH CHECK (sentin8_context_initialized()
               AND env_id = sentin8_current_env_id())';
EXCEPTION WHEN OTHERS THEN
    RAISE NOTICE 'RLS setup failed for regulatory.audit_journal: %', SQLERRM;
END $$;

-- Outbox + journal role grants (same pattern as the table grants above).
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'exchange8_regulatory_service') THEN
        GRANT SELECT, INSERT, UPDATE ON ALL TABLES IN SCHEMA regulatory TO exchange8_regulatory_service;
    END IF;
END $$;
