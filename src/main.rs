// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Exchange8:Regulatory
// SPDX-License-Identifier: Apache-2.0

//! Exchange8:REGULATORY service binary — multi-jurisdiction regulatory reporting.
//!
//! Design: `design/sentin8-exchange8-regulatory-technical-design.md` (see the
//! service doc for the exact file). Core logic in the sibling modules;
//! this shell wires the tokio HTTP/TLS surface via the platform
//! `sentin8-shell` crate (registration, DB pool, health).

#![allow(dead_code)]

// (DESIGN.md §5). Workers (bus consumers, session supervisors, purge jobs)
mod registration;
mod regulatory;

fn extend_router(
    app: axum::Router<std::sync::Arc<sentin8::shell::AppState>>,
    _state: std::sync::Arc<sentin8::shell::AppState>,
) -> axum::Router<std::sync::Arc<sentin8::shell::AppState>> {
    // ── Sentin8:Policy admin API (DESIGN.md §2 — shared implementation;
    // the shell AppState already implements HasPolicyAdmin).
    sentin8::shell::mount_policy_admin(
        // Reporters run on the Bus consumer task (design §3);
        // the HTTP router carries health + admin endpoints only.
        app,
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    sentin8::shell::serve(
        sentin8::shell::Sentin8Shell::new("exchange8:regulatory", "EXCHANGE8_REGULATORY")
            .with_registration(registration::print)
            .with_simple_state()
            .with_router(extend_router),
    )
    .await
}
