# Exchange8:Regulatory - Detailed Design Document

## 1. Scope & Context

**Owns**

- Reporter configuration (`regulatory.regulatory_session`) - per-environment
  links to each reporting regime: jurisdiction, format
  (`fix`/`api`/`file`/`sftp`), transport, address/port, credentials as
  secret reference, enabled flag; unique `(env_id, jurisdiction)`.
- Report history and acknowledgment state (`regulatory.regulatory_report`)
  - one row per formatted report with the dense per-(env, jurisdiction)
  sequence, the report class, the payload and the
  `sent/acked/rejected/retrying/failed` lifecycle.
- The reporter core: `Jurisdiction` (6 regimes), `ReportClass` (5 classes),
  the normalized `ReportableEvent`, the `Reporter` trait with one
  implementation per regime, and `ReportRouter` fan-out with per-jurisdiction
  sequence isolation and a transport-down retry queue.
- The outbox producer for regulatory events (`regulatory.outbox`, notify
  channel `regulatory_outbox_changed`) and the audit journal
  (`regulatory.audit_journal`) for the same writes.

**Does NOT own (delegates)**

- The order/trade/quote stream itself - Exchange8:ME, Exchange8:OMS, Exchange8:Clearing,
  Exchange8:MD produce the events; this service consumes them.
- Fix-session and API transport implementations - regime transports are
  configuration rows; the transport adapters are a connectivity concern
  (Exchange8:VFeeds owns external feed sessions analogously).
- Surveillance detection and market-abuse analysis - Exchange8:Surveillance (MAR
  reporting decisions originate there).
- Derivative instrument/contract masters - Exchange8:RefData.
- Fiat money movement - Bank8:Ledger.
- Identity, M2M registration, Cron schedules - Sentin8:Auth / Sentin8:Cron.

### System context

```mermaid
graph TD
    subgraph vertical["exchange8 vertical (db exchange8)"]
        ME["Exchange8:ME (order events)"]
        OMS["Exchange8:OMS (order lifecycle)"]
        CLEARING["Exchange8:Clearing (trades)"]
        MD["Exchange8:MD (quotes)"]
        SURV["Exchange8:Surveillance (abuse findings)"]
    end

    subgraph regulators["Trust boundary: regulators"]
        IIROC["IIROC FIXROC<br/>(FIX 5.0 session)"]
        CAT["US CAT portal (API)"]
        ESMA["ESMA MiFIR RTS 22 (API)"]
        FCAQ["UK FCA portal (API)"]
        EMIRQ["EMIR RTS 1/2 (XML / API)"]
        CFTC["CFTC Part 43/45 (API)"]
    end

    subgraph platform["Sentin8 platform"]
        AUTH["Sentin8:Auth<br/>(issuer, M2M DCR)"]
        BUSS["Sentin8:Bus<br/>POST /v1/produce"]
        SYSS["Sentin8:Sys<br/>POST /v1/audit + GET /v1/resolve/hostname"]
        SHIELD["Shield (sentin8-policy)"]
        PROXY["Sentin8:Proxy (single ingress)"]
    end

    SVC["Exchange8:Regulatory<br/>exchange8-regulatory"]

    ME -->|"exchange8:me:* events"| SVC
    OMS -->|"order events"| SVC
    CLEARING -->|"trade events"| SVC
    MD -->|"quote events"| SVC
    SURV -->|"finding context"| SVC
    SVC -->|"M2M client_credentials (cert-bound)"| AUTH
    SVC -->|"outbox relay, POST /v1/produce"| BUSS
    SVC -->|"journal push, POST /v1/audit"| SYSS
    SVC -->|"reports: FIX / API / file / SFTP per regime"| IIROC
    SVC --> CAT
    SVC --> ESMA
    SVC --> FCAQ
    SVC --> EMIRQ
    SVC --> CFTC
    SVC -->|"evaluate (Shield)"| SHIELD
    PROXY -->|"SNI exchange8-regulatory.*"| SVC
    SVC -->|"hostname resolve (sentin8:sys:instances:read)"| SYSS
```

### Container view

```mermaid
graph TD
    subgraph bin["exchange8-regulatory binary"]
        subgraph api["API layer"]
            ROUTER["axum router<br/>GET /v1/health · /openapi.json"]
            POLICY["Shield policy-admin mount<br/>sentin8::shell::mount_policy_admin"]
        end
        subgraph core["Pure domain core"]
            REGC["regulatory: Jurisdiction · ReportClass<br/>ReportableEvent → ReportRouter"]
            REPS["Reporter impls: Iiroc · Cat · Mifir<br/>Fca · Emir · DoddFrank"]
        end
        subgraph shell["sentin8::shell (shared)"]
            POOL["pg pool (capped)"]
            RELAYQ["outbox relay → Bus"]
            JOURNALQ["audit pusher → Sys"]
            DRAINQ["graceful drain"]
        end
    end
    PGR[("PostgreSQL 18<br/>db exchange8 · schema regulatory<br/>role exchange8_regulatory_service")]

    ROUTER --> POLICY
    ROUTER --> POOL
    REGC --> REPS
    REGC --> POOL
    POOL --> PGR
    RELAYQ --> POOL
    JOURNALQ --> POOL
    REPS -->|"FormattedReport (IIROC payload)"| WIRE["per-regime wire payload"]
```

## 2. Runtime Architecture

The binary (`src/main.rs`) is a Sentin8 Shell assembly:
`Sentin8Shell::new("exchange8:regulatory", "EXCHANGE8_REGULATORY")` with
registration printing, `with_simple_state()`, and
`with_router(extend_router)` that mounts the Shield policy-admin API. The
DB pool, drain, outbox relay and audit pusher are shell-provided. Report
fan-out runs on a Bus consumer task; the HTTP router carries health and
admin endpoints only.

```mermaid
graph TD
    subgraph tasks["tokio task topology"]
        HTTP["axum HTTP task (TLS, drain)"]
        HEALTH["health handler"]
        POLADM["Shield policy-admin handlers"]
        CONS["bus consumer (reporter driver)"]
        RELAYT["outbox relay loop<br/>LISTEN regulatory_outbox_changed + poll unrelayed"]
        JOURNALT["audit-journal pusher loop<br/>POST /v1/audit"]
    end
    PGT[("db exchange8 / schema regulatory")]
    BUST["Sentin8:Bus"]
    SYST["Sentin8:Sys"]
    RGTS["regulators (FIX / API / file / SFTP)"]

    HTTP --> HEALTH
    HTTP --> POLADM
    CONS --> PGT
    CONS -->|"FormattedReport per jurisdiction"| RGTS
    RELAYT --> PGT
    RELAYT -->|"POST /v1/produce (M2M)"| BUST
    JOURNALT --> PGT
    JOURNALT -->|"POST /v1/audit (M2M)"| SYST
```

### Request family: transaction reporting generation

```mermaid
sequenceDiagram
    autonumber
    participant SRC as Exchange8:ME / OMS / Clearing / MD
    participant B as Sentin8:Bus
    participant H as bus consumer (reporter driver)
    participant C as ReportRouter + Reporter impls
    participant DB as db exchange8 (schema regulatory)
    participant AUTH as Sentin8:Auth

    SRC->>B: domain event (bus stream family)
    H->>AUTH: M2M token (cert-bound client_credentials)
    H->>B: consume (sentin8:bus:consume)
    H->>C: normalized ReportableEvent + transport_up flag
    C->>C: per jurisdiction: seq advance + format()
    alt reportable in regime and transport up
        C-->>H: FormattedReport {jurisdiction, payload, seq}
    else class not reportable in regime
        C-->>H: Err(Rejected(j)) - no retry entry, sequence consumed
    else transport down
        C-->>H: Err(TransportDown(j)) - payload stashed in retry queue
    end
    H->>DB: BEGIN
    H->>DB: INSERT regulatory.regulatory_report (env_id, jurisdiction, seq, report_class, payload, ack_state)
    H->>DB: INSERT regulatory.outbox (event payload) - same tx
    H->>DB: INSERT regulatory.audit_journal - same tx
    H->>DB: COMMIT (UNIQUE env_id,jurisdiction,seq enforced)
```

### Request family: submission to a regime

```mermaid
sequenceDiagram
    autonumber
    participant H as bus consumer (reporter driver)
    participant S as regulatory_session (config row)
    participant R as Regime transport (FIX / API / file / SFTP)
    participant DB as db exchange8 (schema regulatory)

    H->>S: read session (jurisdiction, format, address, port, creds_secret_ref)
    alt session disabled or absent
        S-->>H: no submission path (regime not configured)
    else transport accepts
        H->>R: submit payload (protocol per format: fix | api | file | sftp)
        R-->>H: ACK
        H->>DB: UPDATE regulatory_report SET ack_state='acked'
    else regulator NACKs
        R-->>H: rejected
        H->>DB: UPDATE regulatory_report SET ack_state='rejected', retry_count+1
    else transport error
        R-->>H: unavailable
        H->>DB: UPDATE regulatory_report SET ack_state='retrying', retry_count+1
        Note over H,db: retry drains → fresh format + fresh sequence<br/>(stashed payload re-formatted, never stale-seq replay)
    end
```

### Request family: acknowledgment ingestion

```mermaid
sequenceDiagram
    autonumber
    participant R as Regime (ACK / NACK / gap-fill)
    participant H as bus consumer (ack handler)
    participant DB as db exchange8 (schema regulatory)
    participant A as audit pusher

    R->>H: acknowledgment for (env_id, jurisdiction, seq)
    H->>DB: BEGIN
    H->>DB: SELECT regulatory_report WHERE UNIQUE triple + sent|retrying
    alt found (sent or retrying)
        H->>DB: UPDATE ack_state → 'acked' (ACK) | 'rejected' (NACK)
        H->>DB: INSERT regulatory.audit_journal - same tx
        H->>DB: COMMIT
    else unknown or already terminal
        H-->>H: duplicate/late ack - no state change (idempotent)
    end
    A->>DB: drain unpushed journal rows
    A->>R2: POST /v1/audit (sentin8:sys:audit:write)
```

### Request family: retry / reconciliation

```mermaid
sequenceDiagram
    autonumber
    participant Q as retry queue (transport down)
    participant C as ReportRouter.retry()
    participant DB as db exchange8 (schema regulatory)
    participant R as Regime transport

    Q->>C: drained entries (jurisdiction, stashed payload)
    C->>C: re-format each stashed report with a FRESH sequence
    alt re-format ok and transport recovered
        C->>R: submit (per regime protocol)
        R-->>C: ACK
        C->>DB: report row → ack_state='acked'
    else attempts exhausted
        C->>DB: report row → ack_state='failed', retry_count final
    end
    Note over C,DB: reconciliation backstop: UNIQUE (env_id,jurisdiction,seq)<br/>prevents double submission of a sequence
```

### Request family: admin RFW

```mermaid
sequenceDiagram
    autonumber
    participant L as SSO session (browser)
    participant D as Exchange8:App/Dash RFW host
    participant P8 as Sentin8:Proxy (ingress)
    participant SVCQ as exchange8-regulatory
    participant SH as Shield

    L->>D: open admin page (sessions | reports)
    D->>P8: POST /proxy/exchange8-regulatory/v1/admin/ui/sessions
    P8->>SVCQ: TLS-terminated request + X-Sentin8 context headers
    SVCQ->>SH: evaluate (step 5) vs exchange8:regulatory:policy:read
    alt deny
        SVCQ-->>P8: 403 (fail-closed)
    else allow
        SVCQ-->>P8: RFW page definition
        Note over L,SVCQ: data endpoints /v1/admin/sessions and /v1/admin/reports<br/>serve the same flow (registration-declared)
    end
```

## 3. Data Model

Schema-per-service: database `exchange8`, schema `regulatory`, role
`exchange8_regulatory_service`, DSN env `EXCHANGE8_REGULATORY_DATABASE_URL`.
RLS (`ENABLE` + `FORCE`) on every table; policies require
`sentin8_context_initialized()` and `env_id = sentin8_current_env_id()`
(set via `SELECT sentin8_set_tenant_context(...)` on the same connection,
transaction-local, awaited).

```mermaid
erDiagram
    REGULATORY_SESSION ||--o{ REGULATORY_REPORT : "jurisdiction (per-env unique)"
    REGULATORY_REPORT ||--o{ OUTBOX : "domain write + event, same tx"
    REGULATORY_REPORT ||--o{ AUDIT_JOURNAL : "audited transitions"

    REGULATORY_SESSION {
        uuid id PK
        uuid env_id
        text jurisdiction "iiroc|cat|mifir|fca|emir|dodd_frank"
        text format "fix|api|file|sftp"
        text transport
        text address
        integer port
        text creds_secret_ref "secret ref only"
        boolean enabled
    }
    REGULATORY_REPORT {
        uuid id PK
        uuid env_id
        text jurisdiction "iiroc|cat|mifir|fca|emir|dodd_frank"
        bigint seq "UNIQUE (env_id, jurisdiction)"
        text report_class "new|modify|cancel|trade|quote"
        jsonb payload
        text ack_state "sent|acked|rejected|retrying|failed"
        integer retry_count
        timestamptz ts
    }
    OUTBOX {
        uuid id PK
        uuid env_id
        text event_type
        text stream_family
        jsonb payload
        text publisher_id_token
        text publisher_subject
        boolean relayed
        timestamptz relayed_at
        int relay_attempts "default 0"
        timestamptz created_at
    }
    AUDIT_JOURNAL {
        uuid id PK
        uuid env_id
        text action
        text target_resource_type
        text target_resource_id
        text actor_user_id
        text actor_subject_type
        text severity "default info"
        text correlation_id
        jsonb before_state
        jsonb after_state
        jsonb details
        boolean pushed
        timestamptz pushed_at
        timestamptz created_at
    }
```

Per-table notes:

| Table | Visibility | Notes |
|---|---|---|
| `regulatory.regulatory_session` | env-scoped, RLS FORCE | one link per (env, jurisdiction) - `UNIQUE (env_id, jurisdiction)`; `jurisdiction` CHECK matches the six regime keys; `format` CHECK `fix/api/file/sftp`; credentials only as `creds_secret_ref` |
| `regulatory.regulatory_report` | env-scoped, RLS FORCE | report history + ack lifecycle; `UNIQUE (env_id, jurisdiction, seq)` is the idempotency backstop; `report_class` CHECK mirrors the five `ReportClass` variants; index `idx_exchange8_regreport_env_state (env_id, ack_state)` for the reconciliation sweep |
| `regulatory.outbox` | env-scoped, RLS FORCE | DBX §6.7 producer table; column `id` doubles as `idempotency_key` at Bus `POST /v1/produce`; partial index `idx_regulatory_outbox_unrelayed (created_at) WHERE relayed = false`; trigger `outbox_notify_trigger` → `pg_notify('regulatory_outbox_changed')` |
| `regulatory.audit_journal` | env-scoped, RLS FORCE | DBX §6.8 journal; single combined policy `audit_journal_dml` (USING + WITH CHECK); partial index `idx_regulatory_audit_journal_unpushed (created_at) WHERE pushed = false` |

### Lifecycle: report submission

```mermaid
stateDiagram-v2
    direction LR
    [*] --> sent: formatted + persisted at seq
    sent --> acked: regulator ACK
    sent --> rejected: regulator NACK
    sent --> retrying: transport down / timeout
    retrying --> sent: retry attempt (fresh sequence)
    retrying --> failed: attempts exhausted (retry_count)
    retrying --> acked: recovery ACK
    acked --> [*]
    rejected --> [*]
    failed --> [*]
```

## 4. API Surface

Grouped by audience; the authorization gate for all surfaces is the 8-step
platform pipeline - bearer verify → tenant resolve → feature gate →
**Shield (sentin8-policy, scope-mirror policy set `scope-mirror`,
`origin='platform'`, `mode='enforced'`)** → RLS context → execute → audit.
The HTTP router mounts
health, `/openapi.json` and the Shield policy-admin API (the shared
`Sentin8:Policy` admin mount over `AppState: HasPolicyAdmin`); the handler
action evaluated in step 5 is `exchange8:regulatory:report` (the single
`HANDLER_ACTIONS` entry asserted by the policy wiring tests).

| Audience | Surface | Gate | Emits |
|---|---|---|---|
| System (M2M) | Report generation from bus streams (`exchange8:regulatory:report` action) | scope-mirror permit / `sentin8:sys:admin`, `sentin8:sys:org:admin`, `sentin8:sys:env:admin` | outbox event (domain payload, `regulatory_outbox_changed`) |
| System (M2M) | Regime submission and ACK/NACK ingestion (`exchange8:regulatory:report`) | scope-mirror permit | outbox event + audit journal row, same tx |
| Admin (RFW) | `/v1/admin/ui/sessions` + `/v1/admin/sessions` (page "Reporters", icon `0xe8cc`) | `exchange8:regulatory:read` / role `exchange8:admin` | - |
| Admin (RFW) | `/v1/admin/ui/reports` + `/v1/admin/reports` (page "Report History", icon `0xe873`) | `exchange8:regulatory:read` / role `exchange8:admin` | - |
| Admin | Shield policy-admin API (`exchange8:regulatory:policy:read` / `:write`) | role `exchange8:regulatory:admin` | `sentin8:sys:policy` family |
| Ops | `GET /v1/health`, `/openapi.json` | unauthenticated health; openapi none | - |

Declared error code: `exchange8_report_rejected` (502). Declared admin
service flag: `exchange8-regulatory`; app label "Regulatory" (icon `0xe8eb`).

**Multi-party reporting chain (outbox → Bus → trigger):**

```mermaid
sequenceDiagram
    autonumber
    participant W as Domain write (report row)
    participant DB as db exchange8 (schema regulatory)
    participant R as Shell relay (M2M: exchange8:regulatory)
    participant B as Sentin8:Bus
    participant T as Trigger (owner / Flow / Cron)

    W->>DB: BEGIN - regulatory_report + outbox (+ audit_journal)
    W->>DB: INSERT regulatory.outbox (payload, idempotency = row id)
    W->>DB: COMMIT (pg_notify regulatory_outbox_changed)
    R->>DB: SELECT unrelayed outbox rows (partial index)
    R->>B: POST /v1/produce (sentin8:bus:produce)
    B-->>R: ACK → marks relayed
    B->>T: stream to consumer group (durable, audited)
    alt delivery fails
        B-->>T: retry × max_retries → DLQ
        Note over T: manual replay, idempotency_key dedup
    end
```

```mermaid
sequenceDiagram
    autonumber
    participant W as Domain write (report / ack / retry)
    participant DB as db exchange8
    participant A as audit pusher (M2M: regulatory-audit)
    participant S as Sentin8:Sys

    W->>DB: domain change + INSERT regulatory.audit_journal (same tx)
    A->>DB: SELECT unpushed audit_journal rows (partial index)
    A->>S: POST /v1/audit (sentin8:sys:audit:write)
    S-->>A: 200 → mark pushed
    Note over A,S: Sys outage never drops audit - rows retry until pushed
```

**Event flow (outbox → Bus → trigger):**

```mermaid
flowchart TD
    W["domain write: regulatory_report INSERT/UPDATE<br/>(report generated · acked · rejected · retried)"]
    OX[("regulatory.outbox (relayed = false)")]
    N["trigger outbox_notify_trigger<br/>pg_notify 'regulatory_outbox_changed'"]
    RLY["shell relay loop (M2M: exchange8:regulatory)<br/>sentin8:bus:produce"]
    B(("Sentin8:Bus<br/>POST /v1/produce"))
    TG["consumer group stream"]
    DLQ["Bus DLQ on failure"]

    W -->|"same tx"| OX
    OX --> N
    N --> RLY
    RLY -->|"2xx: relayed = true"| B
    RLY -->|"error: retry backoff<br/>(row stays unrelayed)"| OX
    B --> TG
    TG -->|"delivery failure ×N"| DLQ
```

## 5. Cross-Cutting Concerns

- **Authentication** - Sentin8:Auth is the only issuer. Admin/user access
  uses user tokens; system reporting uses cert-bound M2M
  `client_credentials` tokens (DCR-registered per workload:
  `exchange8:regulatory` + `regulatory` (service client, both spellings
  granted - read/write, bus produce/consume, stat measure);
  `exchange8:regulatory-policy` + `regulatory-policy` (Shield hot-reload
  consumer + stat emitter pair); `regulatory-audit` (audit-journal pusher,
  `sentin8:sys:audit:write`); `exchange8-regulatory-tls` (database-free
  HostContextResolver, `sentin8:sys:instances:read`). DCR is fail-closed:
  only workloads present in the enrolled registry can register.
- **Authorization** - Shield (sentin8-policy) with the declared scope-mirror
  set (`policy_set_name 'scope-mirror'`, `origin='platform'`,
  `mode='enforced'`, `default_enabled=true`): action-granular permits for
  `exchange8:regulatory:report` over the live effective scope set, mirrored
  from the gates handlers evaluate; platform-admin scopes
  (`sentin8:sys:admin`, `sentin8:sys:org:admin`, `sentin8:sys:env:admin`)
  pass; no-set → deny (fail-closed); shadow is the emergency one-UPDATE
  rollback lever only; `token_tp: 'm2m'` never bypasses policy; tenant
  authors may add role-keyed sets (`principal.roles.contains(...)`), and
  forbid beats permit. The enroll-time dry-run gate rejects a set that
  denies its own grants.
- **Tenant resolution + RLS** - every table `ENABLE` + `FORCE ROW LEVEL
  SECURITY`; policies `sentin8_context_initialized() AND env_id =
  sentin8_current_env_id()`; context set with
  `SELECT sentin8_set_tenant_context(...)` (`set_config(..., true)`,
  transaction-local) on the **same connection** as the data statements -
  the async helper must be awaited. Explicit `env_id = $N` predicates stay
  the primary defense; RLS is the backstop. Role
  `exchange8_regulatory_service` carries no `BYPASSRLS`, holds
  `search_path = regulatory, public`, grants only in database `exchange8`,
  zero cross-schema grants.
- **Idempotency** - report dedup: `UNIQUE (env_id, jurisdiction, seq)`;
  relay: `idempotency_key` = outbox row id at Bus `POST /v1/produce`;
  acknowledgment replay: a late/duplicate ACK on a terminal-state row is a
  no-op; outbox retries reuse the row id.
- **Rate limiting / bot protection** - ingress tier is the platform edge
  (Sentin8:Proxy); service-local rate policies ride the Shield policy
  surface (per-tenant sets), not bespoke code.
- **Audit journaling** - every state transition (report generated, acked,
  rejected, retried) writes `regulatory.audit_journal` in the same
  transaction where possible; the pusher drains to `POST /v1/audit`
  (`sentin8:sys:audit:write`, workload `regulatory-audit`); Sys outage
  retries without loss.
- **Drain** - `sentin8::shell` graceful drain on upgrade: HTTP stops
  accepting, in-flight requests finish, the relay/journal pusher checkpoint
  unrelayed/unpushed rows, and Sentin8:Captain `upgrade-instance` swaps the
  binary (rolling, zero-downtime).
- **TLS** - terminated in-process by `sentin8-tls` behind Sentin8:Proxy
  (single ingress, SNI `exchange8-regulatory.{env}.{org}.{tld}`); the
  `exchange8-regulatory-tls` workload resolves host context DB-free via
  Sys `GET /v1/resolve/hostname` (`sentin8:sys:instances:read`) + TTL
  cache. Post-quantum only.
  relay and journal pusher alike.

## 6. Failure Modes & Recovery

### Submission retry after NACK / transport failure

```mermaid
flowchart TD
    A["formatted report at seq"] --> B{"submit to regime transport"}
    B -->|"ACK"| C["ack_state='acked'"]
    B -->|"NACK"| D["ack_state='rejected'<br/>retry_count + 1"]
    B -->|"unavailable / timeout"| E["ack_state='retrying'<br/>retry_count + 1"]
    E --> F{"retry attempt"}
    F -->|"success"| G["ack_state → acked"]
    F -->|"fail again"| H{"attempts exhausted?"}
    H -->|"no"| E
    H -->|"yes"| I["ack_state='failed' (terminal)"]
    J["transport down at format time"] --> K["payload stashed in retry queue"]
    K --> L["Router.retry() drains on recovery<br/>fresh format + fresh sequence"]
    L --> B
```

### Relay / journal retry and DLQ

```mermaid
flowchart TD
    U["regulatory.outbox unrelayed rows<br/>(partial index created_at WHERE relayed=false)"] --> R1["relay attempt: POST /v1/produce"]
    R1 -->|"Bus 2xx"| OK["relayed = true, relayed_at = now()"]
    R1 -->|"error / timeout"| R2["relay_attempts + 1, backoff"]
    R2 --> R3{"exhausted?"}
    R3 -->|"no"| R1
    R3 -->|"yes"| DLQ["surfaces for manual replay<br/>(same idempotency_key = row id)"]
    J["regulatory.audit_journal unpushed rows<br/>(partial index WHERE pushed=false)"] --> P1["POST /v1/audit"]
    P1 -->|"2xx"| OKP["pushed = true, pushed_at = now()"]
    P1 -->|"Sys down"| P2["retry (journal never drops)"]
    P2 --> P1
```

Recovery sequence (crash between write and relay):

```mermaid
sequenceDiagram
    autonumber
    participant W as Domain write
    participant DB as db exchange8
    participant R as Relay (next boot / retry loop)

    W->>DB: BEGIN - report row + outbox + audit_journal
    W->>W: CRASH before relay (tx COMMITTED)
    Note over DB: UNIQUE (env_id,jurisdiction,seq) holds<br/>outbox unrelayed · journal unpushed
    R->>DB: boot: SELECT unrelayed outbox (partial index)
    R->>DB: POST /v1/produce (idempotency_key = row id)
    Note over R: Bus dedupes → no duplicate consumer work
    R->>DB: relayed = true
    R->>DB: audit pusher drains unpushed journal rows
```

| Failure | Behavior |
|---|---|
| Regime transport down (format time) | payload queued in the router retry queue; report row lands `retrying`; drained on recovery with fresh sequence |
| Regulator NACK | `ack_state='rejected'`, sequence consumed (gap is legal), no retry entry |
| Transport error after submit | `ack_state='retrying'`, `retry_count` advanced; retry drains until `acked` or terminal `failed` |
| Relay outage | outbox rows retry with backoff; `idempotency_key` = row id makes replay safe |
| Bus DLQ | consumer delivery failures land in DLQ; manual replay re-produces with the same `idempotency_key` |
| Sys outage | audit journal rows retry (`pushed = false` partial index), never dropped |
| Regime mis-config (unknown jurisdiction key) | INSERT rejected by the CHECK constraint - no silent regime |
| Duplicate ACK | terminal-state no-op - acknowledgment ingestion is idempotent |

## 7. Concurrency & Performance Model

```mermaid
graph LR
    subgraph proc["process: exchange8-regulatory"]
        AX["axum acceptor tasks<br/>(TLS, per-connection)"] --> HC["handlers (health, policy-admin)"]
        CONS["bus consumer task<br/>(reporter driver)"] -->|"drive"| ROUTERQ["ReportRouter"]
        ROUTERQ -->|"drains"| RPT[("regulatory.regulatory_report")]
        ROUTERQ -->|"per-jurisdiction seqs"| SEQ["HashMap(Jurisdiction → seq)"]
        ROUTERQ -->|"transport down"| RTRY["retry queue<br/>Vec[(Jurisdiction, String)]"]
        RELY["relay task<br/>(LISTEN + poll)"] -->|"drains"| OX[("regulatory.outbox")]
        AJP["audit pusher task"] -->|"drains"| AJR[("regulatory.audit_journal")]
        CORE["pure cores (no locks)<br/>Reporter impls · Jurisdiction keys"]
    end
    POOLQ[("pg pool (capped, sentin8_config::pg_pool)")] --> PGS[("PostgreSQL 18 db exchange8")]
    HC --> POOLQ
    CONS --> POOLQ
    AJP --> POOLQ
    RELY --> POOLQ
    RTRY --- CONS
    RELY -->|"M2M POST /v1/produce"| BUSQ[("Sentin8:Bus")]
    AJP -->|"M2M POST /v1/audit"| SYSQ[("Sentin8:Sys")]
    CONS -->|"FIX / API / file / SFTP"| REGQ["regulators"]
```

- **Task layout** - one HTTP acceptor; one bus consumer driving the
  `ReportRouter`; one relay task (LISTEN `regulatory_outbox_changed` +
  periodic unrelayed poll); one audit pusher. The router holds the mutable
  fan-out state (sequences + retry queue) behind the single consumer, so
  no cross-task shared mutable state exists on the hot path.
- **Pool sizing** - the shared `sentin8_config::pg_pool::connect` caps
  connections (default 3, overridable via `SENTIN8_PG_POOL_MAX`) so ~20
  services cannot exhaust PostgreSQL; the relay, journal pusher and
  consumer share the pool; tenant context is set transaction-locally per
  statement set.
- **Hot paths** - per-jurisdiction format + report row insert (single
  UNIQUE-guarded row + outbox row); sequence bookkeeping is a HashMap
  increment; payloads are plain strings. Cache: nothing hot-cached beyond
  the Shield policy sets (hot-reload consumer keeps them fresh) and the
  embedded HostContextResolver TTL cache.
- **Backpressure** - the `relayed = false` and `pushed = false` partial
  indexes bound the relay/pusher scans; relay backoff keeps a Bus outage
  from spinning; the drain path checkpoints before shutdown.

## 8. Invariants, Insecure-Defaults Rejected, Unsafe Rationale

Invariants (always true):

1. Report sequences are isolated per jurisdiction and monotone per (env,
   jurisdiction); a sequence is never reused and never issued for a
   different jurisdiction.
2. A format rejection consumes a sequence (gap) but never a retry entry; a
   transport-down report consumes both; the stashed payload re-formats with
   a FRESH sequence on retry.
3. A report write and its `regulatory.outbox` row (and wherever possible
   `regulatory.audit_journal`) commit in the same transaction.
4. Uniqueness of a submitted report is enforced by
   `UNIQUE (env_id, jurisdiction, seq)` - a relay replay can never create a
   second submission of the same sequence.
5. Acknowledgment ingestion only transitions from `sent`/`retrying`;
   `acked`, `rejected`, and `failed` are terminal.
6. Tenant context and data statements run on the same connection;
   `sentin8_set_tenant_context` is awaited (transaction-local
   `set_config(..., true)`).
7. Every tenant table carries `ENABLE` + `FORCE ROW LEVEL SECURITY` with
   the standard context policies; the service role has no `BYPASSRLS`, no
   cross-schema grants, and exists only in database `exchange8`.
8. Credentials to regimes exist only as `creds_secret_ref` values - never
   plaintext in `regulatory_session` rows.
9. Jurisdiction keys (`iiroc|cat|mifir|fca|emir|dodd_frank`) and report
   classes (`new|modify|cancel|trade|quote`) match the SQL CHECKs exactly -
   wire keys and schema constraints are a single vocabulary.
10. The seeded Shield scope-mirror text covers every step-5 handler action
    (`exchange8:regulatory:report`) and passes the enroll dry-run gate; a
    set that denies its own grants never ships.

Insecure defaults rejected:

- **`danger_accept_invalid_certs` / hand-rolled CA loading** - TLS is
  in-process via `sentin8-tls` (post-quantum, root CA in the OS trust
  store); regulator endpoints use the configured transport with
  secret-referenced credentials.
- **Direct cross-schema/table joins into other services** - cross-service
  data flows are membus → bus → owner API in the access order rule (this
  service declares zero cross-schema grants today).
- **Static superuser service DSN** - role `exchange8_regulatory_service` is
  unprivileged, non-bypass, database-`exchange8`-only.
- **Plaintext regime credentials** - token/credential columns carry
  references only (Secrets), never values.

## 9. Testing Strategy

```mermaid
graph TD
    TP["Exchange8:Regulatory test pyramid"] --> UT["unit (in-crate)"]
    TP --> PWT["policy wiring tests (tests/policy.rs)"]
    TP --> SQL["SQL matrix (tests/sql_matrix.rs)"]
    TP --> E2E["E2E (e2e-ts)"]

    UT --> UT1["jurisdiction keys + Display"]
    UT --> UT2["per-regime field selection<br/>(IIROC all classes; CAT no quotes; MiFIR/FCA/EMIR/DoddFrank trades only)"]
    UT --> UT3["router fan-out + per-jurisdiction sequence isolation"]
    UT --> UT4["rejection consumes seq, no retry entry"]
    UT --> UT5["transport-down queues + retry drain in registration order"]
    UT --> UT6["payload side / short-sale / cross flags"]
    PWT --> P1["no-set deny · shadow never blocks · forbid wins"]
    PWT --> P2["M2M parity · platform-admin scopes · role-keyed tenant sets"]
    PWT --> P3["mirror covers every HANDLER action · enroll dry-run"]
    SQL --> S1["RLS matrix: no context → 0 rows · own env → 1 · cross env → 0"]
    SQL --> S2["FORCE RLS + select policy on every declared init.sql table"]
    E2E --> E1["human-parity flows through BFF only"]
```

- **Unit layer** - the pure reporting core owns all domain behavior:
  fixed jurisdiction keys, per-regime class rejection rules, fan-out to
  every registered reporter, per-jurisdiction sequence isolation
  (sequences advance across transport-down reports and never roll back),
  retry-queue semantics (stale payload re-formatted, drain in registration
  order, accumulation across down batches), and payload field rendering.
- **Policy layer** - proves the real seeded scope-mirror text
  (`registration::SCOPE_MIRROR_POLICIES`, the exact string `vertical
  enroll` upserts) evaluates fail-closed: no-set deny, enforced no-match
  deny, shadow never blocks, M2M parity, platform-admin scopes pass, forbid
  beats permit, role-keyed tenant sets decide, and the enroll dry-run
  gate accepts the declared set.
- **SQL matrix** - live PostgreSQL 18 as the non-bypass `exchange8_probe`
  role (`sql/probe.sql`): policy coverage for every table this service's
  `init.sql` declares, plus the behavioral matrix (no tenant context → 0
  rows silent; insert without context rejected by WITH CHECK; own-env
  context → exactly one probe row; cross-env → 0 rows), all inside one
  rolled-back transaction, skipped silently while offline so
  `cargo test` stays hermetic.
- **Deliberately not tested here** - platform TLS/RLS plumbing itself,
  Sentin8:Bus delivery guarantees, Shield engine internals: covered by
  the platform's own test strategy docs.
- **E2E** - the vertical's `e2e-ts` suite drives the real UI through the
  BFF only (no direct SQL or backend calls).

## 10. Open Questions

- The HTTP surface carries health, `/openapi.json` and the Shield
  policy-admin API; the RFW admin pages declared in registration
  (`sessions`, `reports`) do not yet have mounted data/UI handlers in this
  binary.
- Regime transports are configuration surfaces (`regulatory_session`
  rows: `format fix/api/file/sftp`, `address`, `port`,
  `creds_secret_ref`) without in-tree transport adapters (FIX session
  management, API clients, SFTP drops) in this unit yet.
- Report persistence, outbox event emission and acknowledgment ingestion
  are designed (state machine, UNIQUE seq contract, outbox pattern) but
  not wired: `event_triggers` in registration is empty and no writer
  surfaces exist in the router extension today.
- Per-env reporter registration (which reporters populate the
  `ReportRouter` for an environment) is a boot-wiring decision pending the
  connectivity integration.
- MiCA and MAR regimes (vertical design §2) have no `Jurisdiction` variant
  or SQL CHECK entry - extending the enum, the CHECKs and the reporter set
  is a target-state change when those regimes are added.
- Statement purge / archival policy for `regulatory_report` history and
  the reconciliation view used by admin RFW pages are deferred design
  items.

---

**Architecture & Design:** exchange8:regulatory and its underlying concepts were designed by [Hamid Jahanzad](https://www.linkedin.com/in/hamidjahanzad/).
