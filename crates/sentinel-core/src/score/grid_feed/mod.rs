//! Free real-time grid carbon intensity published by grid operators.
//!
//! Each feed is opt-in, daemon-only, and answers with one value per
//! operator zone that the configured cloud regions map to:
//!
//! - RTE `éCO2mix` (France, `[green.rte_eco2mix]`): direct emissions of
//!   French production, 15 minutes, Licence Ouverte v2.0, no key.
//! - NESO Carbon Intensity API (Great Britain, `[green.neso]`): direct
//!   emissions of generation plus imports, 30 minutes, CC BY 4.0, no key.
//! - Fingrid (Finland, `[green.fingrid]`): production (dataset 266) or
//!   consumption (265) emissions, 3 minutes, CC BY 4.0, free key.
//! - Energinet Energi Data Service (Denmark DK1/DK2, `[green.energinet]`):
//!   consumption emissions, 5 minutes, CC BY 4.0, no key.
//! - REE e·sios (mainland Spain, `[green.ree_esios]`): direct emissions of
//!   peninsular generation, 5 minutes, no commercial use, free token.
//! - Open Electricity (Australia, `[green.open_electricity]`): direct
//!   emissions of regional generation, 5 minutes, CC BY-NC 4.0, optional key.
//! - MAVIR (Hungary, `[green.mavir]`): direct emissions of domestic power
//!   plants per gross generation, 15 minutes, no open licence, no key.
//! - em6 (New Zealand, `[green.em6]`): direct emissions of grid-connected
//!   generation, 30 minutes, all rights reserved, free without key.
//!
//! Electricity Maps wins for any region it also covers.

pub mod config;
#[cfg(feature = "daemon")]
mod em6;
#[cfg(feature = "daemon")]
mod energinet;
#[cfg(feature = "daemon")]
mod esios;
#[cfg(feature = "daemon")]
mod fingrid;
#[cfg(feature = "daemon")]
mod mavir;
#[cfg(feature = "daemon")]
mod neso;
#[cfg(feature = "daemon")]
mod open_electricity;
#[cfg(feature = "daemon")]
mod rte;
#[cfg(feature = "daemon")]
pub mod scraper;
#[cfg(feature = "daemon")]
pub mod state;

pub use config::{FingridDataset, GridFeedConfig, GridFeedKind};
#[cfg(feature = "daemon")]
pub use scraper::spawn_grid_feed_scraper;
#[cfg(feature = "daemon")]
pub use state::GridFeedState;
