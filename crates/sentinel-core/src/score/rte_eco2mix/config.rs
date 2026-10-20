//! Typed configuration for `[green.rte_eco2mix]`.

use std::time::Duration;

/// Base URL of the ODRÉ `OpenDataSoft` Explore API v2.1.
pub const DEFAULT_RTE_ECO2MIX_ENDPOINT: &str = "https://odre.opendatasoft.com/api/explore/v2.1";

/// Default poll interval: the dataset refreshes every 15 minutes, and
/// 900 s spends about 2,900 of the portal's 50,000 monthly calls.
pub const DEFAULT_RTE_POLL_INTERVAL_SECS: u64 = 900;

/// Live French grid intensity from RTE `éCO2mix`.
#[derive(Debug, Clone)]
pub struct RteEco2mixConfig {
    /// Explore API base URL, trailing slashes trimmed.
    pub endpoint: String,
    pub poll_interval: Duration,
    /// Lowercased cloud regions served by the French grid, each given
    /// the same national reading.
    pub regions: Vec<String>,
}
