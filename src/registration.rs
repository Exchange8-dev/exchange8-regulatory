// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Exchange8:Regulatory
// SPDX-License-Identifier: Apache-2.0

//! Service self-registration data for Exchange8:REGULATORY (generated; hand-tune freely).
//!
//! Output via `--print-registration` (see the platform
//! `sentin8-service-self-registration-design.md`).

use sentin8::registration::{
    AdminScope, AdminUi, AdminUiPage, ErrorCode, M2mGrant, PgRole, PolicySetDecl, Registration,
    RelationDecl, SqlMigration,
};

/// The Shield text of the declared scope-mirror set — public so the
/// policy wiring tests (see DESIGN.md §9, policy layer) assert against
/// the REAL seed.
pub const SCOPE_MIRROR_POLICIES: &str = "permit(principal, action == Action::\"exchange8:regulatory:report\", resource) when { principal.scopes.contains(\"exchange8:regulatory:report\") };
permit(principal, action == Action::\"exchange8:regulatory:report\", resource) when { principal.scopes.contains(\"sentin8:sys:admin\") };
permit(principal, action == Action::\"exchange8:regulatory:report\", resource) when { principal.scopes.contains(\"sentin8:sys:org:admin\") };
permit(principal, action == Action::\"exchange8:regulatory:report\", resource) when { principal.scopes.contains(\"sentin8:sys:env:admin\") };";

/// Default declarative policy set (Sentin8:Policy design §3.2/§3.4,
/// seeded by `vertical enroll` as `origin='platform'`, `mode='enforced'`):
/// action-granular scope permits over the LIVE effective scope set — the
/// exact mirror of the gates this service's handlers evaluate in step 5
/// (policy design §4.3). Enforced mode is the standing posture: step 5 is
/// the only gate; shadow is the emergency one-UPDATE rollback lever only.
const POLICIES: &[PolicySetDecl] = &[PolicySetDecl {
    policy_set_name: "scope-mirror",
    description: "Action-granular scope permits for exchange8:regulatory over the LIVE effective scope set. Enforced — step 5 is the only gate.",
    // Shield text lives in SCOPE_MIRROR_POLICIES (single source).
    policies: SCOPE_MIRROR_POLICIES,
    schema: None,
    mode: "enforced",
    default_enabled: true,
}];

/// Default ReBAC relation tuples (design §3.1c) — none declared by this
/// service today; env admins author their own graph via the relations
/// admin API.
const RELATIONS: &[RelationDecl] = &[];

const PG_ROLES: &[PgRole] = &[PgRole {
    scope: "exchange8:regulatory:read,exchange8:regulatory:write",
    role: "exchange8_regulatory_service",
    bypass_rls: false,
    // Grants live in the exchange8 vertical database only (principles §6.6
    // database-per-vertical, schema-per-service).
    database: "exchange8",
    // This service owns all its reads — no cross-schema grants.
    cross_schema_grants: &[],
}];

const ADMIN_SCOPES: &[AdminScope] = &[
    AdminScope::Typed {
        role: "exchange8:admin",
        scope: "exchange8:regulatory:read",
    },
    AdminScope::Typed {
        role: "exchange8:admin",
        scope: "exchange8:regulatory:write",
    },
    // Policy-admin surface (policy design §3.6; principles §6.0b per-service scope).
    AdminScope::Typed {
        role: "exchange8:regulatory:admin",
        scope: "exchange8:regulatory:policy:read",
    },
    AdminScope::Typed {
        role: "exchange8:regulatory:admin",
        scope: "exchange8:regulatory:policy:write",
    },
];

const ERROR_CODES: &[ErrorCode] = &[ErrorCode {
    enum_name: "Exchange8ReportRejected",
    error_code: "exchange8_report_rejected",
    status_code: 502,
}];

const ADMIN_UI_PAGES: &[AdminUiPage] = &[
    AdminUiPage {
        route: "sessions",
        label: "Reporters",
        ui_endpoint: "/v1/admin/ui/sessions",
        data_endpoint: "/v1/admin/sessions",
        icon: "0xe8cc",
    },
    AdminUiPage {
        route: "reports",
        label: "Report History",
        ui_endpoint: "/v1/admin/ui/reports",
        data_endpoint: "/v1/admin/reports",
        icon: "0xe873",
    },
];

/// Workload scope grants (self-registration design §3.11). One entry per
/// workload this service presents a certificate for; `vertical enroll` upserts the
/// rows so the fail-closed registry map admits the registration. Scopes use the
/// full vocabulary form:
/// - `exchange8:regulatory` / `regulatory` — the service's own client (both
///   spellings granted; runtime call sites use the bare short name, the
///   map looks up the exact workload): bus produce/consume + the
///   shared certificate identity.
/// - `exchange8:regulatory-policy` / `regulatory-policy` — Shield hot-reload
///   consumer + stat emitter pair.
/// - `regulatory-audit` — shell audit-journal pusher workload (§6.8; boot 8c).
/// - `exchange8-regulatory-tls` — database-free HostContextResolver resolve workload
///   (TLS-RLS-4: `GET /v1/resolve/hostname`, scope
///   `sentin8:sys:instances:read`).
const M2M_GRANTS: &[M2mGrant] = &[
    M2mGrant {
        workload: "exchange8:regulatory",
        scopes: &[
            "exchange8:regulatory:read",
            "exchange8:regulatory:write",
            "sentin8:bus:produce",
            "sentin8:bus:consume",
            "sentin8:stat:measure",
        ],
    },
    M2mGrant {
        workload: "regulatory",
        scopes: &[
            "exchange8:regulatory:read",
            "exchange8:regulatory:write",
            "sentin8:bus:produce",
            "sentin8:bus:consume",
            "sentin8:stat:measure",
        ],
    },
    M2mGrant {
        workload: "exchange8:regulatory-policy",
        scopes: &["sentin8:bus:consume", "sentin8:stat:measure"],
    },
    M2mGrant {
        workload: "regulatory-policy",
        scopes: &["sentin8:stat:measure"],
    },
    M2mGrant {
        workload: "regulatory-audit",
        scopes: &["sentin8:sys:audit:write"],
    },
    M2mGrant {
        workload: "exchange8-regulatory-tls",
        scopes: &["sentin8:sys:instances:read"],
    },
];

const SQL_1_INIT: &str = include_str!("../sql/init.sql");

fn empty_openapi() -> serde_json::Value {
    serde_json::Value::Null
}

pub fn print(section: Option<String>) -> anyhow::Result<()> {
    let reg = Registration {
        service_type: "exchange8:regulatory",
        subdomain: "exchange8-regulatory",
        health_path: "/v1/health",
        admin_scopes: ADMIN_SCOPES,
        error_codes: ERROR_CODES,
        consent_providers: &[],
        consent_flows: &[],
        admin_ui: Some(AdminUi {
            label: "Regulatory",
            icon: "0xe8eb",
            service_flag: "exchange8-regulatory",
            pages: ADMIN_UI_PAGES,
        }),
        sql_migrations: vec![SqlMigration {
            version: 1,
            name: "init",
            sql: SQL_1_INIT,
        }],
        pg_roles: PG_ROLES,
        audit_findings: None,
        service_actions: None,
        m2m_grants: Some(M2M_GRANTS),
        openapi_url: "/openapi.json",
        openapi: empty_openapi,
        policies: POLICIES,
        relations: RELATIONS,
        event_triggers: &[],
        schedules: &[],
    };
    sentin8::registration::print_registration(section, &reg, empty_openapi)
}
