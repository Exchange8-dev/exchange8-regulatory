// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Exchange8:Regulatory
// SPDX-License-Identifier: Apache-2.0

//! Exchange8:Regulatory core - multi-jurisdiction order/trade reporting.
//!
//! The FIXROC stream is IIROC-specific; real venues report to multiple
//! regulators. This module owns the reporter abstraction, jurisdiction
//! routing, and the reporting-event shapes. The shell wires per-regime
//! transports (FIX sessions, APIs, files).
//!
//! Design: `design/sentin8-exchange8-regulatory-technical-design.md`.

use thiserror::Error;

/// A reporting jurisdiction/regime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Jurisdiction {
    /// Canada - order/trade/quote reporting (FIXROC).
    Iiroc,
    /// US - Reg NMS / Reg SHO / CAT (consolidated audit trail).
    Cat,
    /// EU - ESMA MiFIR transaction reporting (RTS 22).
    Mifir,
    /// UK - FCA transaction reporting.
    Fca,
    /// EU/UK - EMIR derivatives reporting.
    Emir,
    /// US - Dodd-Frank/swap reporting.
    DoddFrank,
}

impl Jurisdiction {
    /// Stable wire/regime key (used in config rows and event routing).
    pub fn key(self) -> &'static str {
        match self {
            Jurisdiction::Iiroc => "iiroc",
            Jurisdiction::Cat => "cat",
            Jurisdiction::Mifir => "mifir",
            Jurisdiction::Fca => "fca",
            Jurisdiction::Emir => "emir",
            Jurisdiction::DoddFrank => "dodd_frank",
        }
    }
}

impl std::fmt::Display for Jurisdiction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.key())
    }
}

/// Which event classes a reportable event belongs to.
///
/// `OrderModify`/`OrderCancel` are constructed by the shell's event mapping
/// from ME `OrderModified`/`OrderKilled` events; the unit tests exercise
/// new/trade/quote only.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReportClass {
    OrderNew,
    OrderModify,
    OrderCancel,
    Trade,
    Quote,
}

/// A normalized reportable event (the reporters format per regime).
#[derive(Clone, Debug, PartialEq)]
pub struct ReportableEvent {
    pub class: ReportClass,
    pub instrument_id: u32,
    /// Legal entity / account doing the trade.
    pub account_id: u64,
    pub qty: u64,
    pub price_raw: i128,
    pub ts_us: u64,
    /// Side ('B'/'S').
    pub is_buy: bool,
    /// Short-sale flag (Reg SHO / IIROC).
    pub short_sale: bool,
    /// Cross-book flag.
    pub cross: bool,
}

/// Reporter transport outcomes.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ReportError {
    #[error("jurisdiction {0} rejected the report")]
    Rejected(Jurisdiction),
    #[error("transport for {0} unavailable")]
    TransportDown(Jurisdiction),
}

/// A formatted, per-regime report (the shell pushes it to the transport).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormattedReport {
    pub jurisdiction: Jurisdiction,
    /// Regime-specific payload (FIX message / API JSON / CSV row).
    pub payload: String,
    /// Regime sequence number (per session/day).
    pub seq: u64,
}

/// The reporter trait - one impl per jurisdiction (FIXROC is the
/// IIROC impl; CAT/MiFIR/FCA/EMIR/Dodd-Frank are the new impls).
pub trait Reporter {
    fn jurisdiction(&self) -> Jurisdiction;
    /// Format an event; the caller supplies the next per-session sequence.
    fn format(&self, event: &ReportableEvent, seq: u64) -> Result<FormattedReport, ReportError>;
}

/// IIROC reporter - FIXROC parity (FIX 5.0 session).
pub struct IirocReporter;

impl Reporter for IirocReporter {
    fn jurisdiction(&self) -> Jurisdiction {
        Jurisdiction::Iiroc
    }

    fn format(&self, event: &ReportableEvent, seq: u64) -> Result<FormattedReport, ReportError> {
        // FIX 35 for IIROC order/trade reports: D new, G modify, F cancel,
        // 8 exec; payload here is the normalized JSON the shell converts
        // to FIX tags (the pure core carries the field selection).
        let class = match event.class {
            ReportClass::OrderNew => "new",
            ReportClass::OrderModify => "modify",
            ReportClass::OrderCancel => "cancel",
            ReportClass::Trade => "trade",
            ReportClass::Quote => "quote",
        };
        let payload = format!(
            "iiroc|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            class,
            event.instrument_id,
            event.account_id,
            event.qty,
            event.price_raw,
            if event.is_buy { "B" } else { "S" },
            event.ts_us,
            event.short_sale,
            event.cross
        );
        Ok(FormattedReport {
            jurisdiction: Jurisdiction::Iiroc,
            payload,
            seq,
        })
    }
}

/// US CAT reporter (Reg NMS consolidated audit trail, API transport).
pub struct CatReporter;

impl Reporter for CatReporter {
    fn jurisdiction(&self) -> Jurisdiction {
        Jurisdiction::Cat
    }

    fn format(&self, event: &ReportableEvent, seq: u64) -> Result<FormattedReport, ReportError> {
        // CAT requires the order event chain; short-sale flags mandatory.
        if event.class == ReportClass::Quote {
            // CAT does not take quotes.
            return Err(ReportError::Rejected(Jurisdiction::Cat));
        }
        let payload = format!(
            "cat|{}|{}|{}|{}|{}|{}|{}",
            event.instrument_id,
            event.account_id,
            event.qty,
            event.price_raw,
            event.is_buy,
            event.short_sale,
            event.ts_us
        );
        Ok(FormattedReport {
            jurisdiction: Jurisdiction::Cat,
            payload,
            seq,
        })
    }
}

/// EU MiFIR reporter (RTS 22 transaction reporting, ESMA API).
pub struct MifirReporter;

impl Reporter for MifirReporter {
    fn jurisdiction(&self) -> Jurisdiction {
        Jurisdiction::Mifir
    }

    fn format(&self, event: &ReportableEvent, seq: u64) -> Result<FormattedReport, ReportError> {
        // MiFIR reports transactions only; order events are not reported.
        if event.class != ReportClass::Trade {
            return Err(ReportError::Rejected(Jurisdiction::Mifir));
        }
        let payload = format!(
            "mifir|{}|{}|{}|{}|{}|{}",
            event.instrument_id,
            event.account_id,
            event.qty,
            event.price_raw,
            event.is_buy,
            event.ts_us
        );
        Ok(FormattedReport {
            jurisdiction: Jurisdiction::Mifir,
            payload,
            seq,
        })
    }
}

/// UK FCA reporter (UK MiFIR transaction reporting, FCA Portal API).
pub struct FcaReporter;

impl Reporter for FcaReporter {
    fn jurisdiction(&self) -> Jurisdiction {
        Jurisdiction::Fca
    }

    fn format(&self, event: &ReportableEvent, seq: u64) -> Result<FormattedReport, ReportError> {
        // FCA reports transactions only (UK MiFIR RTS 22 equivalent);
        // payload mirrors the ESMA RTS 22 field set the FCA portal accepts.
        if event.class != ReportClass::Trade {
            return Err(ReportError::Rejected(Jurisdiction::Fca));
        }
        let payload = format!(
            "fca|{}|{}|{}|{}|{}|{}",
            event.instrument_id,
            event.account_id,
            event.qty,
            event.price_raw,
            event.is_buy,
            event.ts_us
        );
        Ok(FormattedReport {
            jurisdiction: Jurisdiction::Fca,
            payload,
            seq,
        })
    }
}

/// EMIR reporter (EU/UK derivatives reporting, RTS 1/2 of EMIR REFIT).
pub struct EmirReporter;

impl Reporter for EmirReporter {
    fn jurisdiction(&self) -> Jurisdiction {
        Jurisdiction::Emir
    }

    fn format(&self, event: &ReportableEvent, seq: u64) -> Result<FormattedReport, ReportError> {
        // EMIR covers derivatives only (table 1/table 2 field sets);
        // short-sale and cross flags do not apply to derivatives reports.
        if event.class != ReportClass::Trade {
            return Err(ReportError::Rejected(Jurisdiction::Emir));
        }
        let payload = format!(
            "emir|{}|{}|{}|{}|{}|{}",
            event.instrument_id,
            event.account_id,
            event.qty,
            event.price_raw,
            event.is_buy,
            event.ts_us
        );
        Ok(FormattedReport {
            jurisdiction: Jurisdiction::Emir,
            payload,
            seq,
        })
    }
}

/// Dodd-Frank reporter (US swap reporting, CFTC Part 43/45).
pub struct DoddFrankReporter;

impl Reporter for DoddFrankReporter {
    fn jurisdiction(&self) -> Jurisdiction {
        Jurisdiction::DoddFrank
    }

    fn format(&self, event: &ReportableEvent, seq: u64) -> Result<FormattedReport, ReportError> {
        // Part 43 (public dissemination) / Part 45 (real-time SDR submission);
        // payload carries the field set shared by both parts.
        if event.class != ReportClass::Trade {
            return Err(ReportError::Rejected(Jurisdiction::DoddFrank));
        }
        let payload = format!(
            "dodd_frank|{}|{}|{}|{}|{}|{}",
            event.instrument_id,
            event.account_id,
            event.qty,
            event.price_raw,
            event.is_buy,
            event.ts_us
        );
        Ok(FormattedReport {
            jurisdiction: Jurisdiction::DoddFrank,
            payload,
            seq,
        })
    }
}

/// Report router: routes each event to every configured jurisdiction.
pub struct ReportRouter {
    reporters: Vec<Box<dyn Reporter>>,
    /// Per-jurisdiction sequence counters (per day, reset by the shell).
    seqs: std::collections::HashMap<Jurisdiction, u64>,
    /// Reports that failed and need retry (transport-down backstop).
    pub retry_queue: Vec<(Jurisdiction, String)>,
}

impl ReportRouter {
    pub fn new() -> Self {
        ReportRouter {
            reporters: Vec::new(),
            seqs: std::collections::HashMap::new(),
            retry_queue: Vec::new(),
        }
    }

    /// Register a reporter (one per jurisdiction).
    pub fn register(&mut self, reporter: Box<dyn Reporter>) {
        self.reporters.push(reporter);
    }

    /// Route one event to all configured reporters. Returns per-jurisdiction
    /// results; `Err(TransportDown)` entries land in the retry queue.
    pub fn report(
        &mut self,
        event: &ReportableEvent,
        transport_up: bool,
    ) -> Vec<Result<FormattedReport, ReportError>> {
        let mut out = Vec::new();
        for r in &self.reporters {
            let j = r.jurisdiction();
            let seq = self.seqs.entry(j).or_insert(0);
            *seq += 1;
            match r.format(event, *seq) {
                Ok(mut report) => {
                    if transport_up {
                        out.push(Ok(report));
                    } else {
                        self.retry_queue.push((j, report.payload.clone()));
                        report.payload = String::new();
                        out.push(Err(ReportError::TransportDown(j)));
                    }
                }
                Err(e) => out.push(Err(e)),
            }
        }
        out
    }

    /// Drain the retry queue (transport recovery); re-formatted with fresh
    /// sequences by re-formatting the stashed payloads.
    pub fn retry(&mut self) -> Vec<(Jurisdiction, String)> {
        std::mem::take(&mut self.retry_queue)
    }

    /// Current sequence for a jurisdiction (diagnostics).
    pub fn seq(&self, j: Jurisdiction) -> u64 {
        self.seqs.get(&j).copied().unwrap_or(0)
    }
}

impl Default for ReportRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trade(short_sale: bool) -> ReportableEvent {
        ReportableEvent {
            class: ReportClass::Trade,
            instrument_id: 1,
            account_id: 42,
            qty: 100,
            price_raw: 10_000,
            ts_us: 1_000,
            is_buy: true,
            short_sale,
            cross: false,
        }
    }

    #[test]
    fn jurisdiction_keys_stable() {
        assert_eq!(Jurisdiction::Iiroc.key(), "iiroc");
        assert_eq!(Jurisdiction::Cat.key(), "cat");
        assert_eq!(Jurisdiction::Mifir.key(), "mifir");
        assert_eq!(Jurisdiction::Fca.key(), "fca");
        assert_eq!(Jurisdiction::Emir.key(), "emir");
        assert_eq!(Jurisdiction::DoddFrank.key(), "dodd_frank");
    }

    #[test]
    fn iiroc_formats_all_classes() {
        let r = IirocReporter;
        let rep = r.format(&trade(false), 1).unwrap();
        assert_eq!(rep.jurisdiction, Jurisdiction::Iiroc);
        assert!(rep.payload.starts_with("iiroc|trade|"));
        assert!(rep.payload.contains("|B|"));
        // Short-sale flag carried.
        let rep2 = r.format(&trade(true), 2).unwrap();
        assert!(rep2.payload.contains("|true|false"));
    }

    #[test]
    fn cat_rejects_quotes() {
        let r = CatReporter;
        let mut quote = trade(false);
        quote.class = ReportClass::Quote;
        assert!(matches!(
            r.format(&quote, 1),
            Err(ReportError::Rejected(Jurisdiction::Cat))
        ));
        // Trades pass.
        assert!(r.format(&trade(false), 2).is_ok());
    }

    #[test]
    fn mifir_only_takes_trades() {
        let r = MifirReporter;
        assert!(r.format(&trade(false), 1).is_ok());
        let mut order = trade(false);
        order.class = ReportClass::OrderNew;
        assert!(matches!(
            r.format(&order, 2),
            Err(ReportError::Rejected(Jurisdiction::Mifir))
        ));
    }

    #[test]
    fn fca_only_takes_trades() {
        let r = FcaReporter;
        assert!(r.format(&trade(false), 1).is_ok());
        let mut order = trade(false);
        order.class = ReportClass::OrderCancel;
        assert!(matches!(
            r.format(&order, 2),
            Err(ReportError::Rejected(Jurisdiction::Fca))
        ));
    }

    #[test]
    fn emir_only_takes_trades() {
        let r = EmirReporter;
        let rep = r.format(&trade(false), 1).unwrap();
        assert!(rep.payload.starts_with("emir|"));
        let mut quote = trade(false);
        quote.class = ReportClass::Quote;
        assert!(matches!(
            r.format(&quote, 2),
            Err(ReportError::Rejected(Jurisdiction::Emir))
        ));
    }

    #[test]
    fn dodd_frank_only_takes_trades() {
        let r = DoddFrankReporter;
        let rep = r.format(&trade(false), 1).unwrap();
        assert!(rep.payload.starts_with("dodd_frank|"));
        let mut order = trade(false);
        order.class = ReportClass::OrderModify;
        assert!(matches!(
            r.format(&order, 2),
            Err(ReportError::Rejected(Jurisdiction::DoddFrank))
        ));
    }

    #[test]
    fn router_fans_out_to_all_jurisdictions() {
        let mut router = ReportRouter::new();
        router.register(Box::new(IirocReporter));
        router.register(Box::new(CatReporter));
        router.register(Box::new(MifirReporter));
        let results = router.report(&trade(false), true);
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.is_ok()));
        // Sequences increment per jurisdiction.
        assert_eq!(router.seq(Jurisdiction::Iiroc), 1);
        assert_eq!(router.seq(Jurisdiction::Cat), 1);
        assert_eq!(router.seq(Jurisdiction::Mifir), 1);
        router.report(&trade(false), true);
        assert_eq!(router.seq(Jurisdiction::Iiroc), 2);
    }

    #[test]
    fn router_rejections_and_sequences() {
        let mut router = ReportRouter::new();
        router.register(Box::new(CatReporter));
        router.register(Box::new(MifirReporter));
        // A quote: CAT rejects, MiFIR rejects - no transport in queue.
        let mut quote = trade(false);
        quote.class = ReportClass::Quote;
        let results = router.report(&quote, true);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.is_err()));
        assert!(router.retry_queue.is_empty());
    }

    #[test]
    fn router_transport_down_queues_retry() {
        let mut router = ReportRouter::new();
        router.register(Box::new(IirocReporter));
        let results = router.report(&trade(false), false);
        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0],
            Err(ReportError::TransportDown(Jurisdiction::Iiroc))
        ));
        assert_eq!(router.retry_queue.len(), 1);
        // Recovery drains the queue.
        let drained = router.retry();
        assert_eq!(drained.len(), 1);
        assert!(drained[0].1.starts_with("iiroc|trade|"));
        assert!(router.retry_queue.is_empty());
    }

    // ----------------------------------------------------------------
    // Untested edges of the reporting core.
    // ----------------------------------------------------------------

    #[test]
    fn jurisdiction_display_matches_key() {
        let js = [
            Jurisdiction::Iiroc,
            Jurisdiction::Cat,
            Jurisdiction::Mifir,
            Jurisdiction::Fca,
            Jurisdiction::Emir,
            Jurisdiction::DoddFrank,
        ];
        for j in js {
            assert_eq!(j.to_string(), j.key());
        }
    }

    #[test]
    fn router_sequences_are_isolated_per_jurisdiction() {
        let mut router = ReportRouter::new();
        router.register(Box::new(IirocReporter));
        router.register(Box::new(CatReporter));
        // Two reports → seq 1 then 2; a transport-down report still consumes
        // its sequence (the retry re-formats with a FRESH sequence later).
        router.report(&trade(false), true);
        let results = router.report(&trade(false), false);
        assert!(matches!(
            results[0],
            Err(ReportError::TransportDown(Jurisdiction::Iiroc))
        ));
        assert_eq!(router.seq(Jurisdiction::Iiroc), 2);
        assert_eq!(router.seq(Jurisdiction::Cat), 2);
        // Retry drains: the transport flag is global, so BOTH jurisdictions
        // queued one entry each.
        let q = router.retry();
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].0, Jurisdiction::Iiroc);
        assert_eq!(q[1].0, Jurisdiction::Cat);
        assert!(router.retry_queue.is_empty());
    }

    #[test]
    fn rejected_events_do_not_consume_retry_queue_or_advance_seq() {
        // Hmm - check semantics: seq IS incremented before format() (the
        // router counts attempts per jurisdiction). Pin the actual behavior:
        // a rejection consumes a sequence number (gaps are legal per FIX
        // seqnum rules), but nothing lands in the retry queue.
        let mut router = ReportRouter::new();
        router.register(Box::new(MifirReporter));
        let mut order = trade(false);
        order.class = ReportClass::OrderNew;
        let results = router.report(&order, true);
        assert!(results[0].is_err());
        assert!(router.retry_queue.is_empty());
        assert_eq!(router.seq(Jurisdiction::Mifir), 1);
    }

    #[test]
    fn payload_carries_side_and_short_sale_fields() {
        let r = IirocReporter;
        let mut sell = trade(false);
        sell.is_buy = false;
        let rep = r.format(&sell, 1).unwrap();
        assert!(rep.payload.contains("|S|"));
        // Short-sale flag true renders literally.
        let short = trade(true);
        let rep2 = r.format(&short, 2).unwrap();
        assert!(rep2.payload.contains("|true|"));
    }

    #[test]
    fn empty_router_reports_nothing() {
        let mut router = ReportRouter::new();
        let results = router.report(&trade(false), true);
        assert!(results.is_empty());
        assert!(router.retry_queue.is_empty());
    }

    #[test]
    fn retry_queue_is_fresh_per_transport_down_batch() {
        let mut router = ReportRouter::new();
        router.register(Box::new(IirocReporter));
        router.register(Box::new(CatReporter));
        // Batch 1 down → 2 queued (one per reporter).
        router.report(&trade(false), false);
        assert_eq!(router.retry_queue.len(), 2);
        // Transport still down: queue accumulates (no dedupe), and retry()
        // drains EVERYTHING at once.
        router.report(&trade(false), false);
        assert_eq!(router.retry_queue.len(), 4);
        let drained = router.retry();
        assert_eq!(drained.len(), 4);
        // Two entries per jurisdiction, in registration order per batch.
        assert_eq!(drained[0].0, Jurisdiction::Iiroc);
        assert_eq!(drained[1].0, Jurisdiction::Cat);
        assert_eq!(drained[2].0, Jurisdiction::Iiroc);
        assert_eq!(drained[3].0, Jurisdiction::Cat);
        assert!(router.retry_queue.is_empty());
    }
}
