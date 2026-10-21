//! Open Electricity (Australia): emissions (tCO2e) and energy (`MWh`) per
//! region and fuel technology every 5 minutes, for the NEM regions and the
//! WEM. The intensity is their ratio. The key is optional: anonymous calls
//! get the free plan's limits.
//!
//! Timestamps follow AEMO's interval-ending convention: 14:35 covers 14:30
//! to 14:35.

use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;

use super::scraper::{FeedError, check_value, parse_rfc3339};

/// One network per call. Its offset is fixed: `date_start` must be naive
/// network time, and Sydney time would be off by an hour in summer.
struct Network {
    code: &'static str,
    utc_offset_hours: i64,
    /// Region code in the answer -> zone.
    regions: &'static [(&'static str, &'static str)],
}

const NETWORKS: [Network; 2] = [
    Network {
        code: "NEM",
        utc_offset_hours: 10,
        regions: &[
            ("NSW1", "AU-NSW"),
            ("QLD1", "AU-QLD"),
            ("SA1", "AU-SA"),
            ("TAS1", "AU-TAS"),
            ("VIC1", "AU-VIC"),
        ],
    },
    Network {
        code: "WEM",
        utc_offset_hours: 8,
        regions: &[("WEM", "AU-WA")],
    },
];

/// Rooftop solar lands up to an hour and a half behind the rest of the
/// series on the WEM.
const LOOKBACK: TimeDelta = TimeDelta::hours(3);
/// Net storage and loads: counting them would count the same energy twice.
const NOT_GENERATION: [&str; 3] = ["battery", "battery_charging", "pumps"];
const ROOFTOP: &str = "solar_rooftop";

#[derive(Deserialize)]
struct Response {
    success: bool,
    #[serde(default)]
    data: Vec<Series>,
}

#[derive(Deserialize)]
struct Series {
    metric: String,
    results: Vec<SeriesResult>,
}

#[derive(Deserialize)]
struct SeriesResult {
    columns: Columns,
    data: Vec<(String, Option<f64>)>,
}

#[derive(Deserialize)]
struct Columns {
    region: String,
    fueltech: Option<String>,
}

/// One call per network serving a wanted zone, with the zones it serves.
pub(super) fn requests<'a>(
    endpoint: &str,
    wanted: &[&'a str],
    now: DateTime<Utc>,
) -> Vec<(String, Vec<&'a str>)> {
    NETWORKS
        .iter()
        .filter_map(|network| {
            let zones: Vec<&str> = wanted
                .iter()
                .copied()
                .filter(|zone| network.regions.iter().any(|(_, z)| z == zone))
                .collect();
            if zones.is_empty() {
                return None;
            }
            let start = (now + TimeDelta::hours(network.utc_offset_hours) - LOOKBACK)
                .format("%Y-%m-%dT%H:%M:%S");
            let url = format!(
                "{endpoint}/data/network/{}?metrics=energy&metrics=emissions&interval=5m\
                 &primary_grouping=network_region&secondary_grouping=fueltech&date_start={start}",
                network.code
            );
            Some((url, zones))
        })
        .collect()
}

/// Per region: emissions over generation at the newest interval rooftop
/// solar covers, or the newest of any series when the region has none.
/// Reading past it would leave rooftop out and overstate the intensity.
pub(super) fn parse(body: &[u8]) -> Result<Vec<(String, f64, DateTime<Utc>)>, FeedError> {
    let response: Response =
        serde_json::from_slice(body).map_err(|e| FeedError::Parse(e.to_string()))?;
    if !response.success {
        return Err(FeedError::Parse(
            "the answer reports no success".to_string(),
        ));
    }
    let mut zones = Vec::new();
    for (region, zone) in NETWORKS.iter().flat_map(|network| network.regions) {
        let series = |metric: &'static str| {
            response
                .data
                .iter()
                .filter(move |s| s.metric == metric)
                .flat_map(|s| &s.results)
                .filter(move |r| r.columns.region == *region)
        };
        let Some(at) = interval(series("energy"))? else {
            continue;
        };
        let mut emissions = 0.0;
        for result in series("emissions") {
            emissions += value_at(result, at)?.unwrap_or(0.0);
        }
        let mut generation = 0.0;
        for result in series("energy").filter(|r| {
            !r.columns
                .fueltech
                .as_deref()
                .is_some_and(|f| NOT_GENERATION.contains(&f))
        }) {
            generation += value_at(result, at)?.unwrap_or(0.0).max(0.0);
        }
        if generation <= 0.0 {
            continue;
        }
        if let Ok(value) = check_value(Some(1000.0 * emissions / generation)) {
            zones.push(((*zone).to_string(), value, at));
        }
    }
    if zones.is_empty() {
        return Err(FeedError::Empty);
    }
    Ok(zones)
}

/// The interval to read for one region's energy series.
fn interval<'a>(
    energy: impl Iterator<Item = &'a SeriesResult>,
) -> Result<Option<DateTime<Utc>>, FeedError> {
    let mut newest = None;
    for result in energy {
        let Some((raw, _)) = result.data.iter().rev().find(|(_, v)| v.is_some()) else {
            continue;
        };
        let at = parse_rfc3339(raw, "data")?;
        if result.columns.fueltech.as_deref() == Some(ROOFTOP) {
            return Ok(Some(at));
        }
        newest = newest.max(Some(at));
    }
    Ok(newest)
}

fn value_at(result: &SeriesResult, at: DateTime<Utc>) -> Result<Option<f64>, FeedError> {
    for (raw, value) in &result.data {
        if parse_rfc3339(raw, "data")? == at {
            return Ok(*value);
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    /// Metric, region, fueltech and points of one series.
    type Row<'a> = (&'a str, &'a str, &'a str, &'a [(&'a str, Option<f64>)]);

    /// One series per row.
    fn body(series: &[Row]) -> String {
        let metric = |m: &str| {
            let results: Vec<String> = series
                .iter()
                .filter(|(metric, ..)| *metric == m)
                .map(|(_, region, fueltech, points)| {
                    let data: Vec<String> = points
                        .iter()
                        .map(|(t, v)| {
                            format!(r#"["{t}",{}]"#, v.map_or("null".to_string(), |v| v.to_string()))
                        })
                        .collect();
                    format!(
                        r#"{{"name":"{m}_{region}|{fueltech}","columns":{{"region":"{region}","fueltech":"{fueltech}"}},"data":[{}]}}"#,
                        data.join(",")
                    )
                })
                .collect();
            format!(
                r#"{{"network_code":"NEM","metric":"{m}","unit":"x","interval":"5m","results":[{}],"network_timezone_offset":"+10:00"}}"#,
                results.join(",")
            )
        };
        format!(
            r#"{{"version":"4.5.18","success":true,"data":[{},{}]}}"#,
            metric("energy"),
            metric("emissions")
        )
    }

    const T1: &str = "2026-10-20T22:20:00+10:00";
    const T2: &str = "2026-10-20T22:25:00+10:00";

    #[test]
    fn one_call_per_network_with_naive_network_time() {
        let calls = requests(
            "https://api.openelectricity.org.au/v4",
            &["AU-NSW", "AU-VIC", "AU-WA"],
            at("2026-10-20T13:05:00Z"),
        );
        assert_eq!(calls.len(), 2);
        assert!(calls[0].0.starts_with(
            "https://api.openelectricity.org.au/v4/data/network/NEM?metrics=energy&metrics=emissions"
        ));
        assert!(calls[0].0.ends_with("&date_start=2026-10-20T20:05:00"));
        assert_eq!(calls[0].1, vec!["AU-NSW", "AU-VIC"]);
        assert!(calls[1].0.contains("/network/WEM?"));
        assert!(calls[1].0.ends_with("&date_start=2026-10-20T18:05:00"));
        assert_eq!(calls[1].1, vec!["AU-WA"]);
        assert!(calls[1].0.parse::<hyper::Uri>().is_ok());
        let nem_only = requests("https://x", &["AU-SA"], at("2026-10-20T13:05:00Z"));
        assert_eq!(nem_only.len(), 1);
    }

    #[test]
    fn reads_the_newest_interval_rooftop_covers_without_storage() {
        let body = body(&[
            (
                "energy",
                "NSW1",
                "coal_black",
                &[(T1, Some(100.0)), (T2, Some(110.0))],
            ),
            ("energy", "NSW1", "battery_discharging", &[(T1, Some(12.0))]),
            (
                "energy",
                "NSW1",
                "battery",
                &[(T1, Some(7.0)), (T2, Some(7.0))],
            ),
            ("energy", "NSW1", "battery_charging", &[(T1, Some(5.0))]),
            ("energy", "NSW1", "solar_rooftop", &[(T1, Some(50.0))]),
            (
                "emissions",
                "NSW1",
                "coal_black",
                &[(T1, Some(90.0)), (T2, Some(99.0))],
            ),
            ("emissions", "NSW1", "solar_rooftop", &[(T1, Some(0.0))]),
        ]);
        let zones = parse(body.as_bytes()).unwrap();
        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].0, "AU-NSW");
        // 90 t over 100 + 12 + 50 MWh.
        assert!((zones[0].1 - 1000.0 * 90.0 / 162.0).abs() < 1e-9);
        assert_eq!(zones[0].2, at(T1));
    }

    #[test]
    fn without_rooftop_the_newest_interval_is_read_and_zero_is_a_value() {
        let body = body(&[
            (
                "energy",
                "TAS1",
                "hydro",
                &[(T1, Some(70.0)), (T2, Some(77.9))],
            ),
            (
                "emissions",
                "TAS1",
                "hydro",
                &[(T1, Some(0.0)), (T2, Some(0.0))],
            ),
            ("energy", "SA1", "gas_ccgt", &[(T1, Some(10.0)), (T2, None)]),
            ("emissions", "SA1", "gas_ccgt", &[(T1, Some(4.0))]),
        ]);
        let zones = parse(body.as_bytes()).unwrap();
        assert_eq!(zones.len(), 2);
        assert_eq!(zones[0].0, "AU-SA");
        assert!((zones[0].1 - 400.0).abs() < 1e-9);
        assert_eq!(zones[0].2, at(T1));
        assert_eq!(zones[1].0, "AU-TAS");
        assert!(zones[1].1.abs() < f64::EPSILON);
        assert_eq!(zones[1].2, at(T2));
    }

    #[test]
    fn no_generation_is_empty_and_errors_are_parse_errors() {
        let body = body(&[("energy", "VIC1", "battery", &[(T1, Some(3.0))])]);
        assert!(matches!(parse(body.as_bytes()), Err(FeedError::Empty)));
        let refused = r#"{"version":"4.5.18","response_status":"ERROR","error":"Invalid API key","success":false}"#;
        assert!(matches!(
            parse(refused.as_bytes()),
            Err(FeedError::Parse(_))
        ));
        let validation = r#"{"detail":[{"type":"enum","loc":["query","metrics",0],"msg":"x"}]}"#;
        assert!(matches!(
            parse(validation.as_bytes()),
            Err(FeedError::Parse(_))
        ));
    }
}
