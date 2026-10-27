//! End-to-end integration tests for the perf-sentinel CLI.
//!
//! Single test binary. Tests live in per-topic modules and share the
//! helpers module below.

mod helpers;

mod ack;
mod analyze;
#[cfg(feature = "daemon")]
mod capture;
mod demo;
mod diff;
mod disclose;
#[cfg(feature = "pdf")]
mod disclose_pdf;
mod explain;
#[cfg(feature = "daemon")]
mod hub;
mod jaeger_query;
mod mysql_stat;
mod pg_stat;
#[cfg(feature = "daemon")]
mod query;
mod report;
mod running_example;
mod tempo;
mod tui;
mod watch;
