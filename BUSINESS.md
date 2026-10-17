# Exchange8:Regulatory — Business Requirements & Functionality

> Audience: product owners, business analysts, and anyone evaluating what
> Exchange8:Regulatory **does for the business**. For architecture read `README.md`;
> for wire-level detail read `DESIGN.md`.

## 1. Business Purpose

- Keep the venue **compliant with every regulator it answers to**: order,
  trade and quote activity is reported to IIROC (Canada), the US regimes
  (Reg NMS / Reg SHO / CAT), ESMA MiFIR (EU), the UK FCA, and the
  derivatives rule books (EMIR, Dodd-Frank) — from one normalized event
  stream, not six bespoke pipelines.
- Guarantee **complete, ordered, and provable reporting**: every submitted
  report is sequenced per regime and per environment, and the venue can
  show the regulator exactly what was sent, when, and with what outcome.
- Bear the **regime differences in code, not process**: each jurisdiction's
  field-selection rules (what is reportable, which fields are mandatory)
  live in one pluggable reporter per regime.
- Treat the regulator like any other external counterparty: submissions
  have durable state, retries, acknowledgments and a reconciliation trail —
  an outage or a NACK is a workable business event, never a lost record.
- Keep **credentials and transport details** out of code: a regime link is
  environment configuration (endpoint, protocol, secret reference), so
  onboarding a jurisdiction or rotating credentials is data, not a release.

## 2. Business Scope (What the Service Provides)

| # | Capability | Description |
|---|---|---|
| 1 | Multi-jurisdiction fan-out | One normalized reportable event is routed to every configured regime: IIROC/FIXROC, US CAT, MiFIR, FCA, EMIR, Dodd-Frank |
| 2 | Per-regime formatting | Each regime formats events per its own field rules (order/trade/quote for IIROC; consolidated-audit-trade chain and short-sale flags for CAT; trades-only transaction reports for MiFIR/FCA/EMIR/Dodd-Frank) |
| 3 | Regime connectivity configuration | Per-environment links to each regulator: protocol (FIX, API, file, SFTP), transport, address/port, credentials as secret references, enable flag |
| 4 | Report sequencing | A dense report sequence per (environment, jurisdiction), isolating gaps per regime for regulator reconciliation |
| 5 | Report lifecycle tracking | Every report carries sent / acked / rejected / retrying / failed with a durable retry count |
| 6 | Acknowledgment reconciliation | Regulator ACKs and NACKs update the report record; duplicate or late acknowledgments are harmless |
| 7 | Transport-outage resilience | Reports queued during a regime outage are re-formatted with fresh sequences and drained on recovery |
| 8 | Regulatory event publishing | Report outcomes are published to the platform event bus for downstream consumers (audit, surveillance, ops) |
| 9 | Admin surfaces | Two RFW admin pages (Reporters, Report History) registered with the platform admin shell |
| 10 | Audit trail | Every reporting state change is journaled and delivered to the platform audit ingest |

**What the service does NOT own (business boundary)**

| Concern | Owner |
|---|---|
| Order and trade events feeding the reports | Exchange8:ME / Exchange8:OMS / Exchange8:Clearing |
| Quote events feeding the reports | Exchange8:MD |
| Market-abuse detection and MAR analysis | Exchange8:Surveillance |
| Transport sessions to regulators (FIX sessions, API clients, SFTP drops) | Regime transport adapters (connectivity layer) |
| Securities/derivatives reference data | Exchange8:RefData |
| Clearing and settlement of the reported trades | Exchange8:Clearing / Exchange8:Settlement |
| Fiat money movement | Bank8:Ledger |
| Identity, tokens, M2M client registration | Sentin8:Auth |
| Message delivery between services | Sentin8:Bus |
| Scheduling of periodic reporting jobs | Sentin8:Cron |

## 3. Business Capabilities

### Reporting coverage (jurisdictions)

| Capability | Business rule |
|---|---|
| IIROC (Canada) | Order, trade and quote reports in the FIXROC regime — new, modify, cancel, trade and quote classes all format, with side and short-sale flags carried |
| US CAT | Order events and trades report (with mandatory short-sale flags); quotes are not a CAT obligation and are refused |
| MiFIR (EU) | Transaction reporting only — trades format; order events and quotes are refused |
| FCA (UK) | UK transaction reporting mirrors the ESMA field set; trades only |
| EMIR (EU/UK) | Derivatives reporting (table 1/2 field sets); trades only, short-sale and cross flags not applicable |
| Dodd-Frank (US) | Swap reporting (public dissemination and real-time SDR submission field sets); trades only |
| Stable regime keys | Each regime has a fixed identifier used in configuration and events; onboarding a regime means adding a row plus a reporter, never a fork |

### Report production

| Capability | Business rule |
|---|---|
| Normalized events | Order activity, trades and quotes from the venue's engines are normalized into one reportable-event shape (instrument, account, quantity, price, side, short-sale, cross, timestamp) |
| Fan-out | Every event goes to every configured jurisdiction — a venue cannot silently skip a regime it is configured for |
| Sequence discipline | Each jurisdiction numbers its reports independently and monotonically; a rejected event shows as a gap (legal), and no sequence is ever reused |
| Short-sale & cross flags | Short-sale markers and cross-book indicators ride the report where the regime requires them |
| Rejection semantics | An event a regime does not accept (e.g. a quote to CAT) is refused by that regime's reporter only — other regimes still receive it |

### Report lifecycle & reconciliation

| Capability | Business rule |
|---|---|
| Durable lifecycle | Every formatted report is a record with an acknowledgment state and retry count; the record, not memory, is the business truth |
| Acknowledgment | A regulator acknowledgment closes a report as acknowledged; a NACK records the rejection |
| Retry discipline | Transport failures put a report into retrying with an increasing attempt count; only exhausted attempts fail it terminally |
| Sequence safety | The same report (environment, jurisdiction, sequence) can never exist twice, no matter how events are replayed |
| Duplicate-tolerance | A duplicate or late acknowledgment never changes a settled outcome |
| Credential hygiene | Regime credentials are stored only as references to the platform secret store |

### Ops & governance

| Capability | Business rule |
|---|---|
| Enable/disable per regime | A jurisdiction link can be disabled for an environment without code change; a disabled regime produces no submissions |
| Format per regime | Each regime's connectivity declares its protocol family (FIX, API, file, SFTP) as configuration |
| Admin visibility | Reporters and report history are inspectable through the platform admin shell (RFW pages) |
| Event publication | Reporting outcomes fan out on the platform bus so surveillance, audit and ops consume the same truth |

## 4. Business Events (Outbound)

| Event family | When it fires |
|---|---|
| Report generated (`exchange8:regulatory:*`, action `exchange8:regulatory:report`) | A reportable event has been formatted and persisted for a jurisdiction |
| Report acknowledged | A regulator acknowledged a pending report |
| Report rejected | A regulator NACKed a report |
| Report retrying / failed | The transport path failed and retry bookkeeping changed the lifecycle state |

Outbound delivery follows the platform outbox pattern: the reporting record
and its event are written in one transaction, and the platform relay pushes
it to Sentin8:Bus (durable, idempotent). The service declares no inbound
event triggers today; its inputs are the venue's own bus streams (orders,
trades, quotes) and regulator acknowledgments entering via the regime
transports.

## 5. Business Rules & Invariants

1. Every reportable event is offered to **all** configured jurisdictions
   for the environment; no regime is skipped silently.
2. A report sequence is **unique per (environment, jurisdiction)**;
   duplicate submission by replay is structurally impossible.
3. A report that a regime refuses (wrong event class) consumes that
   regime's sequence as a gap — gaps are legal, sequence reuse is not.
4. Reports queued during a transport outage are re-formatted with a **fresh
   sequence** on recovery; a stale queued payload is never replayed as-is.
5. A report's lifecycle is durable: only `sent` and `retrying` rows accept
   acknowledgment transitions; `acked`, `rejected`, and `failed` are
   terminal.
6. Every reporting state change is auditable end-to-end: journal row →
   delivered audit event, with retries after a platform outage and zero
   loss.
7. All reporting data is tenant-isolated: no operation can read or write
   another environment's reports (row-level security backstop on every
   table; explicit environment predicates primary).
8. Regime credentials exist only as secret references — plaintext
   credentials are a design violation, never a configuration option.
9. The service is authorized by the platform policy engine only, and
   fail-closed: no policy set means no access, and machine identities never
   bypass policy.
10. Event and report identifiers use the platform vocabulary
   (`exchange8:regulatory:...` scopes and event names); regimes carry
   stable lowercase keys (`iiroc`, `cat`, `mifir`, `fca`, `emir`,
   `dodd_frank`).

## 6. Target-User Roles & Use Cases

| Persona | Role | What they do with the service |
|---|---|---|
| Compliance officer | Venue compliance | Configures which jurisdictions the venue reports to, per environment; verifies reporting posture |
| Regulator (external) | Recipient | Receives formatted order/trade/quote reports over the agreed protocol and returns ACK/NACK |
| Reporting operations analyst | Ops | Monitors report lifecycle in the admin shell: pending acknowledgments, retries, failures; drills into report history |
| Surveillance officer | Risk & surveillance | Consumes reporting outcomes and regime rejections as context for market-abuse analysis |
| Platform operator | Engineering | Manages the service's lifecycle, credentials (as secret references) and policy sets |
| Exchange8 admin | Admin shell user | Browses the Reporters and Report History admin pages under the platform's admin RBAC |

## 7. KPIs / Success Criteria

- **Reporting completeness** — every reportable event produces exactly one
  attempted report per configured jurisdiction (fan-out coverage 100%).
- **Sequence integrity** — zero duplicate sequences per (env, jurisdiction)
  ever; gaps in a jurisdiction's sequence are explainable (rejections count
  as issued, not lost).
- **Acknowledgment closure** — the share of reports reaching a terminal
  state (acked / rejected / failed) with no reports stuck in `sent` or
  `retrying` beyond the regime's acknowledgment window.
- **Outage recovery** — all reports queued during a regime outage are
  re-formatted and drained on recovery; none lost, none double-sent.
- **Auditability** — 100% of lifecycle transitions journaled and pushed to
  the platform audit intake; a Sys outage never drops an audit record.
- **Tenant isolation** — zero cross-environment reads/writes under the
  enforced RLS matrix.
- **Compliance posture** — every lifecycle-relevant action authorized by
  the platform policy engine; no unauthenticated path beyond health.

## 8. Dependencies & Reliance on Other Services

| Dependency | What the business relies on it for |
|---|---|
| Exchange8:ME / Exchange8:OMS | Order events (new, modify, cancel) that become reportable order activity |
| Exchange8:Clearing | Trade events that become transaction reports |
| Exchange8:MD | Quote events feeding quote-reporting regimes |
| Exchange8:Surveillance | Market-abuse findings that contextualize regulatory output |
| Exchange8:RefData | Instrument reference data the reported instruments resolve against |
| Sentin8:Auth | Identity issuance, M2M client credentials (certificate-bound), admin token verification |
| Sentin8:Bus | Durable, idempotent event delivery for reporting outcomes (outbox producer pattern) |
| Sentin8:Sys | Audit intake (`POST /v1/audit`) and hostname resolution for ingress |
| Sentin8:Secrets | Credential references for regime endpoints (never plaintext) |
| Sentin8:Proxy | Single ingress; TLS termination posture for the service surface |
| Regime transports | The regulator-side endpoints each jurisdiction's configuration points at |

## 9. Acceptance Criteria

1. A trade executed in a fan-out environment produces an attempted report
   to every configured jurisdiction for that environment; a jurisdiction's
   configured absence produces none.
2. The same event replayed twice yields one report row per jurisdiction —
   the sequence uniqueness constraint forbids a second submission.
3. A quote submitted to the US CAT reporter is refused by that reporter
   while IIROC continues to receive it; MiFIR/FCA/EMIR/Dodd-Frank
   reporters accept trades only and refuse order events and quotes.
4. A transport outage during formatting queues the payload; recovery
   drains it with a fresh sequence; no duplicate submission of the original
   sequence occurs.
5. A regulator ACK sets the report to `acked`, a NACK to `rejected`; a
   duplicate ACK on a terminal row changes nothing.
6. Repeated transport failures advance `retry_count` and end terminally in
   `failed` — reports never retry forever outside their state machine.
7. Every lifecycle transition is visible in report history with its
   audited before/after states, and the audit journal drains to the
   platform audit intake even across its outages.
8. A second environment's operator cannot see, query, or submit against
   another environment's report history — enforced by row-level security,
   proven by the RLS matrix tests.
9. All regime credentials resolve through secret references; no plaintext
   credential exists in any reporting configuration row.
10. Admin RBAC: only roles holding the regulatory read/admin scopes can
    open the Reporters or Report History pages, and a caller without any
    applicable scope is denied (fail-closed).

## 10. Open Business Questions

- Which transport each configured regime uses today per environment
  (FIX session, API, file drop, SFTP) and the acknowledgment SLA per
  regulator.
- Whether MAR (market-abuse) and MiCA (crypto-asset) reporting should be
  added as first-class jurisdictions now configured in the regime enum and
  its checks, and which classes they would take.
- The retry budget per regime (when `retrying` becomes terminal `failed`)
  and who owns per-jurisdiction overrides.
- How regulator gap-fill/retransmission requests (e.g. FIX resend) map to
  this service's sequence model and the exchange8 feed recovery pattern.
- Whether report payloads should retain the full per-regime normalized
  form indefinitely or be archived on a retention policy agreed with the
  compliance owner.
- The exact admin data contracts for the Reporters and Report History
  pages (filters, columns, exports) pending the admin-surface wiring.

---

**Design attribution:** exchange8:regulatory and its underlying concepts were designed by Hamid Jahanzad <Hamid@Jahanzad.com>.
