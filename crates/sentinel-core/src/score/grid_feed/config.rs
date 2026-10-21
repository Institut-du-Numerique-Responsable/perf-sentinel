//! Typed configuration shared by the grid operator feeds.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::score::carbon::RealTimeProvider;

/// Which dataset `[green.fingrid]` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FingridDataset {
    /// Dataset 266, emissions of electricity production in Finland, the
    /// basis RTE `éCO2mix` uses.
    Production,
    /// Dataset 265, emissions of electricity consumed in Finland, imports
    /// counted with fixed yearly factors.
    Consumption,
}

impl FingridDataset {
    #[must_use]
    pub const fn id(self) -> u16 {
        match self {
            Self::Production => 266,
            Self::Consumption => 265,
        }
    }
}

/// One grid operator feed. Every variant answers with one value per
/// operator zone, read by the same scraper loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridFeedKind {
    RteEco2mix,
    Neso,
    Fingrid(FingridDataset),
    Energinet,
    ReeEsios,
    OpenElectricity,
    Mavir,
    Em6,
}

impl GridFeedKind {
    #[must_use]
    pub const fn provider(self) -> RealTimeProvider {
        match self {
            Self::RteEco2mix => RealTimeProvider::RteEco2mix,
            Self::Neso => RealTimeProvider::Neso,
            Self::Fingrid(_) => RealTimeProvider::Fingrid,
            Self::Energinet => RealTimeProvider::Energinet,
            Self::ReeEsios => RealTimeProvider::ReeEsios,
            Self::OpenElectricity => RealTimeProvider::OpenElectricity,
            Self::Mavir => RealTimeProvider::Mavir,
            Self::Em6 => RealTimeProvider::Em6,
        }
    }

    /// Config table name, for messages.
    #[must_use]
    pub const fn section(self) -> &'static str {
        match self {
            Self::RteEco2mix => "[green.rte_eco2mix]",
            Self::Neso => "[green.neso]",
            Self::Fingrid(_) => "[green.fingrid]",
            Self::Energinet => "[green.energinet]",
            Self::ReeEsios => "[green.ree_esios]",
            Self::OpenElectricity => "[green.open_electricity]",
            Self::Mavir => "[green.mavir]",
            Self::Em6 => "[green.em6]",
        }
    }

    /// API base URL used when the section sets no `endpoint`.
    #[must_use]
    pub const fn default_endpoint(self) -> &'static str {
        match self {
            Self::RteEco2mix => "https://odre.opendatasoft.com/api/explore/v2.1",
            Self::Neso => "https://api.carbonintensity.org.uk",
            Self::Fingrid(_) => "https://data.fingrid.fi/api",
            Self::Energinet => "https://api.energidataservice.dk",
            Self::ReeEsios => "https://api.esios.ree.es",
            Self::OpenElectricity => "https://api.openelectricity.org.au/v4",
            Self::Mavir => "https://rtdwweb.mavir.hu/rtdwweb/webuser",
            Self::Em6 => "https://api.em6.co.nz/ords/em6/data_api",
        }
    }

    /// Poll interval used when the section sets none: about the operator's
    /// refresh step, within each portal's quota.
    #[must_use]
    pub const fn default_poll_secs(self) -> u64 {
        match self {
            Self::RteEco2mix => 900,
            Self::Fingrid(_) => 180,
            Self::Neso | Self::Energinet | Self::ReeEsios | Self::Mavir | Self::Em6 => 300,
            // Two calls per poll when both networks are mapped: 288 a day,
            // within the 500 of the free plan.
            Self::OpenElectricity => 600,
        }
    }

    /// Operator zones the feed can serve.
    #[must_use]
    pub const fn zones(self) -> &'static [&'static str] {
        match self {
            Self::RteEco2mix => &["FR"],
            Self::Neso => &["GB"],
            Self::Fingrid(_) => &["FI"],
            Self::Energinet => &["DK1", "DK2"],
            Self::ReeEsios => &["ES"],
            Self::OpenElectricity => &["AU-NSW", "AU-QLD", "AU-SA", "AU-TAS", "AU-VIC", "AU-WA"],
            Self::Mavir => &["HU"],
            Self::Em6 => &["NZ"],
        }
    }
}

/// A configured grid operator feed.
#[derive(Clone)]
pub struct GridFeedConfig {
    pub kind: GridFeedKind,
    /// API base URL, trailing slashes trimmed.
    pub endpoint: String,
    pub poll_interval: Duration,
    /// Lowercased cloud region -> operator zone (`FR`, `DK1`, `AU-NSW`...).
    pub regions: BTreeMap<String, String>,
    /// Fingrid and REE e·sios need one, sent as `x-api-key`; Open
    /// Electricity takes an optional one as a bearer token.
    pub api_key: Option<String>,
}

impl std::fmt::Debug for GridFeedConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GridFeedConfig")
            .field("kind", &self.kind)
            .field("endpoint", &self.endpoint)
            .field("poll_interval", &self.poll_interval)
            .field("regions", &self.regions)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_the_api_key() {
        let cfg = GridFeedConfig {
            kind: GridFeedKind::Fingrid(FingridDataset::Production),
            endpoint: "https://data.fingrid.fi/api".to_string(),
            poll_interval: Duration::from_mins(3),
            regions: BTreeMap::from([("europe-north1".to_string(), "FI".to_string())]),
            api_key: Some("s3cr3t-key".to_string()),
        };
        let shown = format!("{cfg:?}");
        assert!(!shown.contains("s3cr3t-key"), "{shown}");
        assert!(shown.contains("<redacted>"));
    }
}
