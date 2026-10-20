//! RTE `éCO2mix` integration for real-time French grid carbon intensity.
//!
//! Polls the public `eco2mix-national-tr` dataset that RTE publishes on
//! the ODRÉ open data portal (Licence Ouverte v2.0, no key). Its
//! `taux_co2` is an estimate of the direct emissions of electricity
//! production in France, refreshed every 15 minutes: a free real-time
//! source for French regions, ranked below Electricity Maps when both
//! are configured.

pub mod config;
#[cfg(feature = "daemon")]
pub mod scraper;
#[cfg(feature = "daemon")]
pub mod state;

pub use config::RteEco2mixConfig;
#[cfg(feature = "daemon")]
pub use scraper::spawn_rte_eco2mix_scraper;
#[cfg(feature = "daemon")]
pub use state::RteEco2mixState;
