//! Validation of the `[green]` section and its energy-backend subsections.

use std::collections::{BTreeMap, HashMap};

use crate::score::alumet::AlumetConfig;
use crate::score::cloud_energy::config::{CloudEnergyConfig, ServiceCloudConfig};
use crate::score::host_static::{HostPlacement, StaticHostsConfig, Subsystem};
use crate::score::kepler::{KeplerConfig, KeplerMetricKind};
use crate::score::redfish::{RedfishConfig, RedfishEndpoint};
use crate::score::scaphandre::ScaphandreConfig;

use super::{
    Config, check_range, has_control_char, validate_http_authority, validate_workload_fields,
    warn_unknown_region,
};

/// The tail of a load-time warning for an instance type the embedded
/// table cannot price: `None` when it resolves to a row, empty when it
/// is unknown, `": <reason>"` when it names a class left out on purpose.
pub(crate) fn unpriced_instance_suffix(instance_type: &str) -> Option<String> {
    use crate::score::cloud_energy::table::{InstanceClass, resolve_instance_class};
    match resolve_instance_class(instance_type) {
        InstanceClass::Known { .. } => None,
        InstanceClass::Unsupported(reason) => Some(format!(": {reason}")),
        InstanceClass::Unknown => Some(String::new()),
    }
}

/// One message per CPU query read by services whose resolved power
/// envelopes differ (rule C1), naming the first conflicting pair in
/// sorted order. Such a query names no single node, so the scraper
/// prices each of its services alone.
pub(crate) fn mixed_query_envelope_conflicts(cfg: &CloudEnergyConfig) -> Vec<String> {
    let mut out = Vec::new();
    for (query, services) in cfg.cpu_query_groups() {
        let mut members = services
            .iter()
            .map(|name| (name, cfg.service_power(&cfg.services[name])));
        let Some((first, envelope)) = members.next() else {
            continue;
        };
        if let Some((other, other_envelope)) = members.find(|(_, e)| *e != envelope) {
            out.push(format!(
                "[green.cloud.services] '{first}' ({} to {} W) and '{other}' ({} to {} W) \
                 read the same CPU query '{query}' with different power envelopes, so each \
                 is priced alone on the full reading instead of sharing one node. Give each \
                 node a cpu_query filtered to it, or one instance type for the services of \
                 one node",
                envelope.0, envelope.1, other_envelope.0, other_envelope.1
            ));
        }
    }
    out
}

fn warn_mixed_query_envelopes(cfg: &CloudEnergyConfig) {
    for message in mixed_query_envelope_conflicts(cfg) {
        tracing::warn!("{message}");
    }
}

/// Scheme, host, port and chassis path of a Redfish endpoint URL.
type RedfishChassisKey = (String, String, u16, String);

const REDFISH_CHASSIS_PREFIX: &str = "/redfish/v1/Chassis/";

/// Chassis identity of an endpoint URL (rule R1): scheme and host
/// lowercased, the port or the scheme default, and the path cut to
/// `/redfish/v1/Chassis/<id>` so `/Power`, `/EnvironmentMetrics` and
/// `/Sensors/<x>` of one chassis collide. Other paths keep their full
/// form without a trailing slash. Parsed by hand like
/// `validate_http_authority`, which has already accepted the URL,
/// because `hyper` is optional outside the daemon features.
fn redfish_chassis_key(url: &str) -> RedfishChassisKey {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let scheme = scheme.to_ascii_lowercase();
    let (authority, path) = rest.find('/').map_or((rest, ""), |i| rest.split_at(i));
    let default_port = if scheme == "https" { 443 } else { 80 };
    // `]` closes a bracketed IPv6 host, so a colon after it starts the port.
    let host_end = authority.rfind(']').map_or(0, |i| i + 1);
    let (host, port) = match authority[host_end..].rfind(':') {
        Some(i) => authority.split_at(host_end + i),
        None => (authority, ""),
    };
    let port = port
        .strip_prefix(':')
        .and_then(|p| p.parse().ok())
        .unwrap_or(default_port);
    let path = path
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_end_matches('/');
    let path_key = match path.strip_prefix(REDFISH_CHASSIS_PREFIX) {
        Some(rest) if !rest.is_empty() => {
            let id = rest.split('/').next().unwrap_or(rest);
            format!("{REDFISH_CHASSIS_PREFIX}{id}")
        }
        _ => path.to_string(),
    };
    (scheme, host.to_ascii_lowercase(), port, path_key)
}

/// A node-level reading and the services it is attributed to.
type NodeGroup = (String, Vec<String>);

/// `service -> key` inverted to `key -> sorted services`.
fn groups_by_value(section: &str, map: &HashMap<String, String>) -> Vec<NodeGroup> {
    let mut groups: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (service, key) in map {
        groups.entry(key).or_default().push(service.clone());
    }
    groups
        .into_iter()
        .map(|(key, mut services)| {
            services.sort();
            (format!("{section} '{key}'"), services)
        })
        .collect()
}

/// Node-level groups ranked by precedence, lowest first: cloud CPU
/// queries, Redfish chassis, then Alumet domains (only under
/// `label_key = "domain"`, the one Alumet key that names a node).
fn node_level_groups(green: &crate::config::GreenConfig) -> [Vec<NodeGroup>; 3] {
    let cloud = green.cloud_energy.as_ref().map_or_else(Vec::new, |cfg| {
        cfg.cpu_query_groups()
            .into_iter()
            .map(|(query, services)| (format!("[green.cloud] CPU query '{query}'"), services))
            .collect()
    });
    let redfish = green.redfish.as_ref().map_or_else(Vec::new, |cfg| {
        groups_by_value("[green.redfish] chassis", &cfg.service_mappings)
    });
    let alumet = green
        .alumet
        .as_ref()
        .filter(|cfg| cfg.label_key == "domain")
        .map_or_else(Vec::new, |cfg| {
            groups_by_value("[green.alumet] domain", &cfg.service_mappings)
        });
    [cloud, redfish, alumet]
}

/// Messages for node-level readings that overlap without containment
/// (rule O1). A lower group X sharing services with a higher group Y,
/// with members outside Y, still splits its node among those members
/// while Y bills the same node again.
pub(crate) fn overlapping_node_readings(green: &crate::config::GreenConfig) -> Vec<String> {
    let ranks = node_level_groups(green);
    let mut out = Vec::new();
    for (i, lower) in ranks.iter().enumerate() {
        for higher in &ranks[i + 1..] {
            push_overlaps(lower, higher, &mut out);
        }
    }
    out
}

fn push_overlaps(lower: &[NodeGroup], higher: &[NodeGroup], out: &mut Vec<String>) {
    for (x_label, x_services) in lower {
        for (y_label, y_services) in higher {
            let (shared, outside): (Vec<&str>, Vec<&str>) = x_services
                .iter()
                .map(String::as_str)
                .partition(|s| y_services.iter().any(|y| y == s));
            if shared.is_empty() || outside.is_empty() {
                continue;
            }
            out.push(format!(
                "{x_label} ({}) and {y_label} ({}) share {}: {} keep a share of the node \
                 {x_label} reads while {y_label} bills it again. Map every service of that \
                 node to {y_label}, or each service to the one node it runs on",
                x_services.join(", "),
                y_services.join(", "),
                shared.join(", "),
                outside.join(", "),
            ));
        }
    }
}

/// Rule B2: host count and instance type within bounds.
fn validate_host_counts(section: &str, cfg: &StaticHostsConfig) -> Result<(), String> {
    if let Some(nodes) = cfg.nodes
        && !(1..=10_000).contains(&nodes)
    {
        return Err(format!("{section} nodes = {nodes} must be 1 to 10000"));
    }
    if let Some(instance_type) = cfg.instance_type.as_deref()
        && (instance_type.is_empty()
            || instance_type.len() > 256
            || has_control_char(instance_type))
    {
        return Err(format!(
            "{section} instance_type must be 1-256 chars and free of control characters"
        ));
    }
    Ok(())
}

/// Rule B3: the instance type resolves to an embedded row. Runs after
/// [`validate_host_counts`], so the type is printable.
fn validate_host_class(section: &str, cfg: &StaticHostsConfig) -> Result<(), String> {
    use crate::score::cloud_energy::table::{InstanceClass, resolve_instance_class};
    let Some(instance_type) = cfg.instance_type.as_deref() else {
        return Ok(());
    };
    match resolve_instance_class(instance_type) {
        InstanceClass::Known { .. } => Ok(()),
        InstanceClass::Unsupported(reason) => Err(format!(
            "{section} instance_type '{instance_type}' cannot be priced: {reason}"
        )),
        InstanceClass::Unknown => Err(format!(
            "{section} instance_type '{instance_type}' is neither an embedded row nor a \
             managed class that resolves to one. Use a listed type or managed class, see \
             docs/INSTANCE-TYPES.md#managed-service-classes"
        )),
    }
}

/// Rule B4: a dedicated declaration names its hosts as a pair, or only
/// its placement next to the Alumet workload.
fn validate_dedicated_hosts(
    subsystem: Subsystem,
    cfg: &StaticHostsConfig,
    alumet_present: bool,
) -> Result<(), String> {
    let section = subsystem.section();
    if !cfg.shared_with.is_empty() {
        return Err(format!(
            "{section} shared_with applies to placement = 'shared' only: dedicated hosts \
             share no application node"
        ));
    }
    match (cfg.nodes.is_some(), cfg.instance_type.is_some()) {
        (true, true) => Ok(()),
        (false, false) if alumet_present && cfg.cpu_query.is_none() => Ok(()),
        (false, false) => Err(format!(
            "{section} placement = 'dedicated' needs nodes and instance_type. Placement \
             alone is accepted only next to {} and without cpu_query",
            subsystem.alumet_section()
        )),
        _ => Err(format!(
            "{section} nodes and instance_type go together: set both, or neither"
        )),
    }
}

/// Rule B5: a shared host is one application node read by its own query.
fn validate_shared_host(subsystem: Subsystem, cfg: &StaticHostsConfig) -> Result<(), String> {
    let section = subsystem.section();
    if cfg.cpu_query.is_none() {
        return Err(format!(
            "{section} placement = 'shared' needs cpu_query, the {}'s CPU as a percent \
             of the node it shares",
            subsystem.noun()
        ));
    }
    if cfg.nodes.is_some_and(|nodes| nodes != 1) {
        return Err(format!(
            "{section} placement = 'shared' is one node: drop nodes or set it to 1. A \
             workload spread over several application nodes cannot be declared shared"
        ));
    }
    if cfg.instance_type.is_none() && cfg.shared_with.is_empty() {
        return Err(format!(
            "{section} placement = 'shared' needs instance_type when shared_with is \
             empty: no cloud service gives the node's power envelope"
        ));
    }
    Ok(())
}

impl Config {
    /// Rules B2 to B5 and B8/B9 for one subsystem's declared hosts.
    fn validate_static_hosts(&self, subsystem: Subsystem) -> Result<(), String> {
        let alumet = self.green.alumet_host(subsystem);
        if let Some(cfg) = self.green.static_hosts(subsystem) {
            let section = subsystem.section();
            validate_host_counts(section, cfg)?;
            validate_host_class(section, cfg)?;
            match cfg.placement {
                HostPlacement::Dedicated => {
                    validate_dedicated_hosts(subsystem, cfg, alumet.is_some())?;
                }
                HostPlacement::Shared => validate_shared_host(subsystem, cfg)?,
            }
        }
        self.validate_host_region(subsystem)
    }

    /// Rules B8 and B9: a valid declared region, one region per
    /// subsystem, and an effective one whenever a source is configured.
    fn validate_host_region(&self, subsystem: Subsystem) -> Result<(), String> {
        let declared = self
            .green
            .static_hosts(subsystem)
            .map(|cfg| cfg.region.as_deref());
        let alumet = self.green.alumet_host(subsystem);
        if declared.is_none() && alumet.is_none() {
            return Ok(());
        }
        let section = subsystem.section();
        if let Some(region) = declared.flatten() {
            if region.is_empty() || !crate::score::carbon::is_valid_region_id(region) {
                return Err(format!(
                    "{section} region '{region}' is not a valid region id"
                ));
            }
            warn_unknown_region(section, Some(region));
        }
        if let (Some(region), Some((_, Some(alumet_region)))) = (declared.flatten(), alumet)
            && !region.eq_ignore_ascii_case(alumet_region)
        {
            return Err(format!(
                "{section} region '{region}' and {} region '{alumet_region}' differ: the \
                 {} hosts have one region, keep one of the two",
                subsystem.alumet_section(),
                subsystem.noun()
            ));
        }
        if self.green.host_region(subsystem).is_none() {
            return Err(format!(
                "the {} host energy needs a region to convert to gCO2: set region in {section} \
                 or {}, or [green] default_region",
                subsystem.noun(),
                subsystem.alumet_section()
            ));
        }
        Ok(())
    }
}

impl Config {
    pub(super) fn validate_green(&self) -> Result<(), String> {
        Self::validate_embodied_carbon(self.green.embodied_carbon_per_request_gco2)?;
        Self::validate_default_region(self.green.default_region.as_deref())?;
        Self::validate_service_regions(&self.green.service_regions)?;
        if let Some(cfg) = &self.green.scaphandre {
            Self::validate_scaphandre(cfg)?;
        }
        if let Some(cfg) = &self.green.kepler {
            Self::validate_kepler(cfg)?;
        }
        if let Some(cfg) = &self.green.alumet {
            Self::validate_alumet(cfg)?;
        }
        if let Some(cfg) = &self.green.redfish {
            Self::validate_redfish(cfg)?;
        }
        if let Some(cfg) = &self.green.cloud_energy {
            Self::validate_cloud_energy(cfg)?;
        }
        self.validate_static_hosts(Subsystem::Broker)?;
        self.validate_hourly_profiles_file()?;
        if let Some(cfg) = &self.green.electricity_maps {
            Self::validate_electricity_maps(cfg)?;
        }
        Self::validate_grid_feeds(&self.green.grid_feeds)?;
        self.warn_overlapping_node_readings();
        Ok(())
    }

    fn warn_overlapping_node_readings(&self) {
        for message in overlapping_node_readings(&self.green) {
            tracing::warn!("{message}");
        }
    }

    fn validate_embodied_carbon(value: f64) -> Result<(), String> {
        if !value.is_finite() {
            return Err(format!(
                "embodied_carbon_per_request_gco2 must be finite, got {value}"
            ));
        }
        if value < 0.0 {
            return Err(format!(
                "embodied_carbon_per_request_gco2 must be >= 0.0, got {value}"
            ));
        }
        Ok(())
    }

    /// Validate the optional `[green] default_region`. Config is trusted
    /// input, so typos surface loudly here rather than silently producing
    /// zeroed CO₂ rows downstream. Same validator used at the OTLP
    /// ingestion boundary (there, invalid values are silently dropped).
    fn validate_default_region(region: Option<&str>) -> Result<(), String> {
        let Some(region) = region else {
            return Ok(());
        };
        if crate::score::carbon::is_valid_region_id(region) {
            return Ok(());
        }
        Err(format!(
            "[green] default_region '{region}' contains invalid characters; \
             expected ASCII alphanumeric + '-' or '_', length 1-64"
        ))
    }

    /// Validate the `[green.service_regions]` map: cardinality cap, plus
    /// region-id syntax on every key/value pair.
    fn validate_service_regions(map: &HashMap<String, String>) -> Result<(), String> {
        /// Maximum number of entries in `[green.service_regions]`.
        /// Bounds the config-load memory footprint against fat-finger or
        /// malicious configs. 1024 is 4× `MAX_REGIONS` (256) and comfortably
        /// above any realistic multi-cloud deployment size.
        const MAX_SERVICE_REGIONS: usize = 1024;
        if map.len() > MAX_SERVICE_REGIONS {
            return Err(format!(
                "[green.service_regions] has {} entries; maximum is {MAX_SERVICE_REGIONS}",
                map.len()
            ));
        }
        for (service, region) in map {
            if !crate::score::carbon::is_valid_region_id(service) {
                return Err(format!(
                    "[green.service_regions] invalid service name '{service}'; \
                     expected ASCII alphanumeric + '-' or '_', length 1-64"
                ));
            }
            if !crate::score::carbon::is_valid_region_id(region) {
                return Err(format!(
                    "[green.service_regions] invalid region '{region}' for service '{service}'; \
                     expected ASCII alphanumeric + '-' or '_', length 1-64"
                ));
            }
        }
        Ok(())
    }

    /// Validate `[green] hourly_profiles_file`: reject control characters
    /// in the path (log injection) and require that the file loaded when
    /// the field is configured.
    fn validate_hourly_profiles_file(&self) -> Result<(), String> {
        let Some(path) = &self.green.hourly_profiles_file else {
            return Ok(());
        };
        if has_control_char(path) {
            return Err("[green] hourly_profiles_file contains control characters".to_string());
        }
        if self.green.custom_hourly_profiles.is_none() {
            return Err(format!(
                "[green] hourly_profiles_file '{path}' was configured but \
                 failed to load. Remove the field to use embedded profiles only."
            ));
        }
        Ok(())
    }

    /// Validate a parsed `[green.electricity_maps]` config section.
    pub(in crate::config) fn validate_electricity_maps(
        cfg: &crate::score::electricity_maps::ElectricityMapsConfig,
    ) -> Result<(), String> {
        if cfg.auth_token.is_empty() {
            return Err(
                "[green.electricity_maps] api_key or PERF_SENTINEL_EMAPS_TOKEN is required"
                    .to_string(),
            );
        }
        if has_control_char(&cfg.auth_token) {
            return Err(
                "[green.electricity_maps] auth token contains control characters".to_string(),
            );
        }
        validate_http_authority(&cfg.api_endpoint, "[green.electricity_maps] endpoint")?;
        // Warn (but do not fail) when a non-empty auth token travels to an
        // http:// endpoint. The Electricity Maps production API is served
        // over https in practice. An http:// endpoint usually means a local
        // test server or a misconfiguration. Flag it so users do not
        // silently ship credentials in cleartext.
        if cfg.api_endpoint.starts_with("http://") && !cfg.auth_token.is_empty() {
            tracing::warn!(
                "[green.electricity_maps] auth token will be sent over http:// \
                 (no TLS). Use https:// for production or set the endpoint to \
                 a loopback/private address if this is intentional."
            );
        }
        let secs = cfg.poll_interval.as_secs();
        check_range(
            "[green.electricity_maps] poll_interval_secs",
            &secs,
            &60,
            &86400,
        )?;
        if cfg.region_map.is_empty() {
            return Err(
                "[green.electricity_maps] region_map must contain at least one entry".to_string(),
            );
        }
        for (region, zone) in &cfg.region_map {
            if zone.is_empty() {
                return Err(format!(
                    "[green.electricity_maps.region_map] zone for '{region}' is empty"
                ));
            }
            if has_control_char(zone)
                || zone.contains('&')
                || zone.contains('#')
                || zone.contains('=')
                || zone.contains('?')
                || zone.contains('%')
                || zone.contains(' ')
                || zone.contains('+')
            {
                return Err(format!(
                    "[green.electricity_maps.region_map] zone '{zone}' for '{region}' \
                     contains invalid characters"
                ));
            }
            if has_control_char(region) {
                return Err(format!(
                    "[green.electricity_maps.region_map] region key '{region}' \
                     contains control characters"
                ));
            }
        }
        Ok(())
    }

    /// Validate the grid operator feed sections. A cloud region may be
    /// served by one feed only, so the merge order never decides silently.
    pub(in crate::config) fn validate_grid_feeds(
        feeds: &[crate::score::grid_feed::GridFeedConfig],
    ) -> Result<(), String> {
        let mut owner: HashMap<&str, &str> = HashMap::new();
        for feed in feeds {
            let section = feed.kind.section();
            if !feed.endpoint.starts_with("http://") && !feed.endpoint.starts_with("https://") {
                return Err(format!(
                    "{section} endpoint '{}' must start with 'http://' or 'https://'",
                    feed.endpoint
                ));
            }
            validate_http_authority(&feed.endpoint, &format!("{section} endpoint"))?;
            let secs = feed.poll_interval.as_secs();
            check_range(&format!("{section} poll_interval_secs"), &secs, &60, &86400)?;
            if feed.regions.is_empty() {
                return Err(format!(
                    "{section} must map at least one cloud region to the operator's grid"
                ));
            }
            if let Some(key) = &feed.api_key {
                // The rule the header sender applies, so a bad key fails here
                // rather than silently disabling the feed at startup.
                if !key.bytes().all(|b| b.is_ascii_graphic()) {
                    return Err(format!(
                        "{section} API key must be printable ASCII without spaces"
                    ));
                }
                if feed.endpoint.starts_with("http://") {
                    tracing::warn!(
                        "{section} API key will be sent over http:// (no TLS). Use https:// \
                         for production or set the endpoint to a loopback/private address \
                         if this is intentional."
                    );
                }
            }
            for (region, zone) in &feed.regions {
                if region.is_empty() || has_control_char(region) {
                    return Err(format!(
                        "{section} region {region:?} is empty or contains control characters"
                    ));
                }
                if !feed.kind.zones().contains(&zone.as_str()) {
                    return Err(format!(
                        "{section} zone {zone:?} for region {region:?} is not one of {:?}",
                        feed.kind.zones()
                    ));
                }
                if let Some(other) = owner.insert(region, section) {
                    return Err(format!(
                        "region {region:?} is listed in both {other} and {section}; \
                         keep it in the feed of the grid that serves it"
                    ));
                }
            }
        }
        Ok(())
    }

    /// Validate a parsed `[green.scaphandre]` config section.
    ///
    /// Rejects: empty endpoint, non-`http://` scheme, credentials in
    /// authority, control characters, invalid port, `scrape_interval_secs`
    /// outside [1, 3600], and `process_map` keys/values that are empty,
    /// >256 chars, or contain control characters.
    fn validate_scaphandre(cfg: &ScaphandreConfig) -> Result<(), String> {
        if cfg.endpoint.is_empty() {
            return Err(
                "[green.scaphandre] endpoint is required when the section is present".to_string(),
            );
        }
        if !cfg.endpoint.starts_with("http://") && !cfg.endpoint.starts_with("https://") {
            return Err(format!(
                "[green.scaphandre] endpoint '{}' must start with 'http://' or 'https://'",
                cfg.endpoint
            ));
        }
        validate_http_authority(&cfg.endpoint, "[green.scaphandre] endpoint")?;
        let secs = cfg.scrape_interval.as_secs();
        if !(1..=3600).contains(&secs) {
            return Err(format!(
                "[green.scaphandre] scrape_interval_secs must be in [1, 3600], got {secs}"
            ));
        }
        Self::validate_scaphandre_process_map(cfg)?;
        // The `AuthHeader` type lives in the `ingest` module, which is
        // only compiled when hyper is pulled in via one of the daemon /
        // tempo / jaeger-query features. Bare `cargo publish` builds
        // `sentinel-core` with no features and must skip the parse.
        #[cfg(any(feature = "daemon", feature = "tempo", feature = "jaeger-query"))]
        if let Some(auth) = cfg.auth_header.as_deref() {
            crate::ingest::auth_header::AuthHeader::parse(auth)
                .map_err(|msg| format!("[green.scaphandre] auth_header: {msg}"))?;
        }
        Ok(())
    }

    /// Validate a parsed `[green.kepler]` config section.
    ///
    /// Same shape as [`Self::validate_scaphandre`]: rejects empty
    /// endpoints, non-`http(s)` schemes, embedded credentials, control
    /// chars, invalid ports, `scrape_interval_secs` outside [1, 3600],
    /// a blank `zone`, and `service_mappings` keys/values outside
    /// [1, 256] chars or with control chars.
    pub(in crate::config) fn validate_kepler(cfg: &KeplerConfig) -> Result<(), String> {
        if cfg.endpoint.is_empty() {
            return Err(
                "[green.kepler] endpoint is required when the section is present".to_string(),
            );
        }
        if !cfg.endpoint.starts_with("http://") && !cfg.endpoint.starts_with("https://") {
            return Err(format!(
                "[green.kepler] endpoint '{}' must start with 'http://' or 'https://'",
                cfg.endpoint
            ));
        }
        validate_http_authority(&cfg.endpoint, "[green.kepler] endpoint")?;
        let secs = cfg.scrape_interval.as_secs();
        if !(1..=3600).contains(&secs) {
            return Err(format!(
                "[green.kepler] scrape_interval_secs must be in [1, 3600], got {secs}"
            ));
        }
        Self::validate_kepler_zone(&cfg.zone)?;
        Self::validate_kepler_service_mappings(cfg)?;
        #[cfg(any(feature = "daemon", feature = "tempo", feature = "jaeger-query"))]
        if let Some(auth) = cfg.auth_header.as_deref() {
            crate::ingest::auth_header::AuthHeader::parse(auth)
                .map_err(|msg| format!("[green.kepler] auth_header: {msg}"))?;
        }
        Ok(())
    }

    /// The zone is matched verbatim against the `zone` label. No fixed
    /// set applies: hwmon zones are named after the host's sensors.
    /// Surrounding whitespace could never match a label, so it is
    /// rejected rather than left to fail silently.
    fn validate_kepler_zone(zone: &str) -> Result<(), String> {
        if has_control_char(zone) {
            return Err("[green.kepler] zone contains control characters".to_string());
        }
        if zone.trim().is_empty() {
            return Err(format!(
                "[green.kepler] zone '{zone}' is blank; remove the field for the default 'package'"
            ));
        }
        if zone.trim() != zone {
            return Err(format!(
                "[green.kepler] zone '{zone}' has surrounding whitespace, which no label carries"
            ));
        }
        if zone.chars().count() > 256 {
            return Err("[green.kepler] zone is longer than 256 characters".to_string());
        }
        Ok(())
    }

    /// Validate `[green.kepler].service_mappings` keys and values.
    /// Label cap depends on `metric_kind`: 256 for `Container` (full
    /// `container_name`), 15 for `Process` since the kernel truncates
    /// `comm` at `TASK_COMM_LEN - 1`. The cap is `len()` bytes, not
    /// chars, matching the kernel's byte-bounded truncation.
    fn validate_kepler_service_mappings(cfg: &KeplerConfig) -> Result<(), String> {
        /// Memory-footprint cap, mirrors `MAX_SERVICE_REGIONS`.
        const MAX_KEPLER_SERVICE_MAPPINGS: usize = 1024;
        if cfg.service_mappings.len() > MAX_KEPLER_SERVICE_MAPPINGS {
            return Err(format!(
                "[green.kepler] service_mappings has {} entries; maximum is {MAX_KEPLER_SERVICE_MAPPINGS}",
                cfg.service_mappings.len()
            ));
        }
        let (max_label_len, label_hint) = match cfg.metric_kind {
            KeplerMetricKind::Container => (256_usize, ""),
            KeplerMetricKind::Process => (
                15_usize,
                " (the Linux kernel truncates `comm` to 15 bytes, \
                  provide the truncated value, not the full binary path)",
            ),
        };
        for (service, label) in &cfg.service_mappings {
            // Reject control chars first so an ANSI-laden label is not
            // echoed back to stderr via the length-error `format!`.
            if has_control_char(service) {
                return Err("[green.kepler] service_mappings has a service name \
                     that contains control characters"
                    .to_string());
            }
            if has_control_char(label) {
                return Err(format!(
                    "[green.kepler] service_mappings has a label \
                     for service '{service}' that contains control characters"
                ));
            }
            if service.is_empty() || service.len() > 256 {
                return Err(format!(
                    "[green.kepler] service_mappings service name '{service}' must be 1-256 chars"
                ));
            }
            if label.is_empty() || label.len() > max_label_len {
                return Err(format!(
                    "[green.kepler] service_mappings label for service '{service}' \
                     must be 1-{max_label_len} chars, got '{label}'{label_hint}"
                ));
            }
        }
        Ok(())
    }

    /// Validate a parsed `[green.alumet]` config section.
    ///
    /// Beyond the shared endpoint and interval checks, this guards the
    /// two operator-supplied parser inputs (`metric_name`, `label_key`)
    /// and `energy_interval_secs`, the value that silently rescales
    /// every reading when it drifts from the Alumet-side
    /// `poll_interval`. Neither string reaches a Prometheus label (they
    /// are matched against the scraped body), so there is no cardinality
    /// exposure here.
    pub(super) fn validate_alumet(cfg: &AlumetConfig) -> Result<(), String> {
        if cfg.endpoint.is_empty() {
            return Err(
                "[green.alumet] endpoint is required when the section is present".to_string(),
            );
        }
        if !cfg.endpoint.starts_with("http://") && !cfg.endpoint.starts_with("https://") {
            return Err(format!(
                "[green.alumet] endpoint '{}' must start with 'http://' or 'https://'",
                cfg.endpoint
            ));
        }
        validate_http_authority(&cfg.endpoint, "[green.alumet] endpoint")?;
        let secs = cfg.scrape_interval.as_secs();
        if !(1..=3600).contains(&secs) {
            return Err(format!(
                "[green.alumet] scrape_interval_secs must be in [1, 3600], got {secs}"
            ));
        }
        Self::validate_alumet_parser_field(&cfg.metric_name, "metric_name")?;
        Self::validate_alumet_parser_field(&cfg.label_key, "label_key")?;
        let interval = cfg.energy_interval_secs;
        if !interval.is_finite() || interval <= 0.0 || interval > 3600.0 {
            return Err(format!(
                "[green.alumet] energy_interval_secs must be a finite value in (0, 3600], \
                 got {interval}"
            ));
        }
        Self::validate_alumet_service_mappings(cfg)?;
        if let Some(db) = &cfg.database {
            Self::validate_workload_declaration(
                "[green.alumet.database]",
                "database waste",
                &db.label_value,
                db.region.as_deref(),
                cfg,
            )?;
        }
        if let Some(broker) = &cfg.broker {
            Self::validate_workload_declaration(
                "[green.alumet.broker]",
                "messaging waste",
                &broker.label_value,
                broker.region.as_deref(),
                cfg,
            )?;
            // One cgroup cannot be both workloads either: that would
            // count the same joules in two separate figures.
            if cfg
                .database
                .as_ref()
                .is_some_and(|db| db.label_value == broker.label_value)
            {
                return Err(format!(
                    "[green.alumet.broker] label_value '{}' is also declared as the \
                     database workload; one cgroup cannot feed both waste figures",
                    broker.label_value
                ));
            }
        }
        #[cfg(any(feature = "daemon", feature = "tempo", feature = "jaeger-query"))]
        if let Some(auth) = cfg.auth_header.as_deref() {
            crate::ingest::auth_header::AuthHeader::parse(auth)
                .map_err(|msg| format!("[green.alumet] auth_header: {msg}"))?;
        }
        Ok(())
    }

    /// Shared checks of one declared workload (database or broker):
    /// field validity, no collision with `service_mappings`, and a warn
    /// on a region absent from the embedded table. Charset-valid but
    /// unknown regions are legitimate (custom ids covered by Electricity
    /// Maps), so warn instead of rejecting.
    fn validate_workload_declaration(
        section: &str,
        figure: &str,
        label_value: &str,
        region: Option<&str>,
        cfg: &AlumetConfig,
    ) -> Result<(), String> {
        validate_workload_fields(section, label_value, region)?;
        if cfg.service_mappings.values().any(|v| v == label_value) {
            return Err(format!(
                "{section} label_value '{label_value}' also appears in \
                 service_mappings; one cgroup cannot feed both the energy \
                 totals and the {figure} figure"
            ));
        }
        warn_unknown_region(section, region);
        Ok(())
    }

    /// Bound one of the two operator-supplied parser inputs. Control
    /// chars are rejected before the value reaches an error message.
    fn validate_alumet_parser_field(value: &str, field: &str) -> Result<(), String> {
        /// Prometheus metric and label names are far shorter than this
        /// in practice. The cap only bounds the memory a hostile config
        /// can pin per scrape.
        const MAX_PARSER_FIELD_LEN: usize = 256;
        if has_control_char(value) {
            return Err(format!(
                "[green.alumet] {field} contains control characters"
            ));
        }
        if value.is_empty() || value.len() > MAX_PARSER_FIELD_LEN {
            return Err(format!(
                "[green.alumet] {field} must be 1-{MAX_PARSER_FIELD_LEN} chars, got '{value}'"
            ));
        }
        Ok(())
    }

    /// Validate `[green.alumet].service_mappings` keys and values.
    /// Label values are Alumet label values (a pod name, a cgroup id, a
    /// RAPL domain), all well under the shared 256-byte cap.
    fn validate_alumet_service_mappings(cfg: &AlumetConfig) -> Result<(), String> {
        /// Memory-footprint cap, mirrors `MAX_KEPLER_SERVICE_MAPPINGS`.
        const MAX_ALUMET_SERVICE_MAPPINGS: usize = 1024;
        if cfg.service_mappings.len() > MAX_ALUMET_SERVICE_MAPPINGS {
            return Err(format!(
                "[green.alumet] service_mappings has {} entries; maximum is {MAX_ALUMET_SERVICE_MAPPINGS}",
                cfg.service_mappings.len()
            ));
        }
        for (service, label) in &cfg.service_mappings {
            // Reject control chars first so an ANSI-laden label is not
            // echoed back to stderr via the length-error `format!`.
            if has_control_char(service) {
                return Err("[green.alumet] service_mappings has a service name \
                     that contains control characters"
                    .to_string());
            }
            if has_control_char(label) {
                return Err(format!(
                    "[green.alumet] service_mappings has a label \
                     for service '{service}' that contains control characters"
                ));
            }
            if service.is_empty() || service.len() > 256 {
                return Err(format!(
                    "[green.alumet] service_mappings service name '{service}' must be 1-256 chars"
                ));
            }
            if label.is_empty() || label.len() > 256 {
                return Err(format!(
                    "[green.alumet] service_mappings label for service '{service}' \
                     must be 1-256 chars, got '{label}'"
                ));
            }
        }
        Ok(())
    }

    /// Validate a parsed `[green.redfish]` config section.
    ///
    /// Enforces the BMC-specific scrape-interval lower bound
    /// (`MIN_SCRAPE_INTERVAL_SECS`), checks every endpoint URL, walks
    /// the service mapping for control chars + length bounds, ensures
    /// every mapped chassis exists in `endpoints`, and confirms that
    /// the `ca_bundle_path` file is readable when set.
    pub(in crate::config) fn validate_redfish(cfg: &RedfishConfig) -> Result<(), String> {
        use crate::score::redfish::config::{MAX_SCRAPE_INTERVAL_SECS, MIN_SCRAPE_INTERVAL_SECS};
        if cfg.endpoints.is_empty() {
            return Err(
                "[green.redfish] endpoints must contain at least one chassis when the section is present"
                    .to_string(),
            );
        }
        Self::validate_redfish_endpoints(&cfg.endpoints)?;
        let secs = cfg.scrape_interval.as_secs();
        if !(MIN_SCRAPE_INTERVAL_SECS..=MAX_SCRAPE_INTERVAL_SECS).contains(&secs) {
            return Err(format!(
                "[green.redfish] scrape_interval_secs must be in [{MIN_SCRAPE_INTERVAL_SECS}, {MAX_SCRAPE_INTERVAL_SECS}], got {secs}. \
                 The lower bound defends against BMC rate-limit retaliation."
            ));
        }
        Self::validate_redfish_service_mappings(&cfg.service_mappings, &cfg.endpoints)?;
        if let Some(bundle) = cfg.ca_bundle_path.as_deref()
            && bundle.is_empty()
        {
            return Err("[green.redfish] ca_bundle_path must be non-empty when set".to_string());
        }
        // No filesystem probe on `ca_bundle_path`: the scraper task
        // refuses to start the moment the field is set (see
        // `score/redfish/scraper.rs`), so a metadata() check here would
        // only add a path-probe attack surface for no operator benefit
        // until custom-CA TLS lands.
        #[cfg(any(feature = "daemon", feature = "tempo", feature = "jaeger-query"))]
        if let Some(auth) = cfg.auth_header.as_deref() {
            crate::ingest::auth_header::AuthHeader::parse(auth)
                .map_err(|msg| format!("[green.redfish] auth_header: {msg}"))?;
        }
        Ok(())
    }

    /// Validate each `chassis_id -> RedfishEndpoint` pair in
    /// `[green.redfish.endpoints]`, in sorted id order, and reject a
    /// second id that resolves a chassis already declared (rule R1):
    /// its power would be counted once per id. The `schema` field is
    /// type-checked by serde at deserialization, so only the URL needs
    /// runtime validation here.
    fn validate_redfish_endpoints(
        endpoints: &HashMap<String, RedfishEndpoint>,
    ) -> Result<(), String> {
        let mut ids: Vec<&String> = endpoints.keys().collect();
        ids.sort();
        let mut seen: HashMap<RedfishChassisKey, &str> = HashMap::new();
        for chassis_id in ids {
            let url = &endpoints[chassis_id].url;
            Self::validate_redfish_endpoint(chassis_id, url)?;
            if let Some(first) = seen.insert(redfish_chassis_key(url), chassis_id) {
                return Err(format!(
                    "[green.redfish] endpoints '{first}' and '{chassis_id}' read the same \
                     chassis ('{url}'), so its power would be counted once per id. Declare \
                     the chassis once and map its services to that id"
                ));
            }
        }
        Ok(())
    }

    fn validate_redfish_endpoint(chassis_id: &str, url: &str) -> Result<(), String> {
        if chassis_id.is_empty() || chassis_id.len() > 256 {
            return Err(format!(
                "[green.redfish] endpoints chassis id '{chassis_id}' must be 1-256 chars"
            ));
        }
        if has_control_char(chassis_id) {
            return Err(format!(
                "[green.redfish] endpoints chassis id '{chassis_id}' contains control characters"
            ));
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(format!(
                "[green.redfish] endpoint URL for chassis '{chassis_id}' must start with 'http://' or 'https://', got '{url}'"
            ));
        }
        validate_http_authority(
            url,
            &format!("[green.redfish] endpoint URL for chassis '{chassis_id}'"),
        )
    }

    /// Validate each `service -> chassis_id` pair in `[green.redfish.service_mappings]`.
    /// Every mapped chassis must already be declared in `endpoints`.
    fn validate_redfish_service_mappings(
        service_mappings: &HashMap<String, String>,
        endpoints: &HashMap<String, RedfishEndpoint>,
    ) -> Result<(), String> {
        for (service, chassis_id) in service_mappings {
            if service.is_empty() || service.len() > 256 {
                return Err(format!(
                    "[green.redfish] service_mappings service name '{service}' must be 1-256 chars"
                ));
            }
            if has_control_char(service) {
                return Err(format!(
                    "[green.redfish] service_mappings service name '{service}' contains control characters"
                ));
            }
            if !endpoints.contains_key(chassis_id) {
                return Err(format!(
                    "[green.redfish] service '{service}' maps to chassis '{chassis_id}' which is not declared in [green.redfish.endpoints]"
                ));
            }
        }
        Ok(())
    }

    /// Validate `[green.scaphandre].process_map` keys and values.
    ///
    /// Service names (keys), `exe_contains` substrings and optional
    /// `cmdline_contains` substrings must be 1 to 256 chars and free
    /// of control characters. Service names are NOT run through
    /// `is_valid_region_id` because they may legitimately contain dots,
    /// slashes and similar.
    fn validate_scaphandre_process_map(cfg: &ScaphandreConfig) -> Result<(), String> {
        for (service, matcher) in &cfg.process_map {
            Self::validate_scaphandre_substring(service, "service name", service)?;
            Self::validate_scaphandre_substring(&matcher.exe_contains, "exe_contains", service)?;
            if let Some(cmdline) = matcher.cmdline_contains.as_deref() {
                Self::validate_scaphandre_substring(cmdline, "cmdline_contains", service)?;
            }
        }
        Ok(())
    }

    /// Length and control-char validation for one `process_map` string
    /// field. Extracted so [`validate_scaphandre_process_map`] stays
    /// below the cognitive-complexity ceiling. `kind` is the field
    /// label inserted into the error message (e.g. `"exe_contains"`),
    /// `service` is the surrounding service name used for operator
    /// context.
    fn validate_scaphandre_substring(value: &str, kind: &str, service: &str) -> Result<(), String> {
        if value.is_empty() || value.len() > 256 {
            return Err(format!(
                "[green.scaphandre] process_map {kind} for service '{service}' \
                 must be 1-256 chars, got '{value}'"
            ));
        }
        if has_control_char(value) {
            return Err(format!(
                "[green.scaphandre] process_map {kind} for service '{service}' \
                 contains control characters"
            ));
        }
        Ok(())
    }

    /// Validate a parsed `[green.cloud]` config section.
    fn validate_cloud_energy(cfg: &CloudEnergyConfig) -> Result<(), String> {
        Self::validate_cloud_endpoint(cfg)?;
        Self::validate_cloud_services(cfg)?;
        // See the twin note in `validate_scaphandre`: the `AuthHeader`
        // type is feature-gated, so bare no-features builds skip it.
        #[cfg(any(feature = "daemon", feature = "tempo", feature = "jaeger-query"))]
        if let Some(auth) = cfg.auth_header.as_deref() {
            crate::ingest::auth_header::AuthHeader::parse(auth)
                .map_err(|msg| format!("[green.cloud] auth_header: {msg}"))?;
        }
        Ok(())
    }

    /// Validate `[green.cloud]` endpoint, scrape interval, provider, and instance type.
    fn validate_cloud_endpoint(cfg: &CloudEnergyConfig) -> Result<(), String> {
        if cfg.prometheus_endpoint.is_empty() {
            return Err(
                "[green.cloud] prometheus_endpoint is required when the section is present"
                    .to_string(),
            );
        }
        if !cfg.prometheus_endpoint.starts_with("http://")
            && !cfg.prometheus_endpoint.starts_with("https://")
        {
            return Err(format!(
                "[green.cloud] prometheus_endpoint '{}' must start with 'http://' or 'https://'",
                cfg.prometheus_endpoint
            ));
        }
        validate_http_authority(
            &cfg.prometheus_endpoint,
            "[green.cloud] prometheus_endpoint",
        )?;
        let secs = cfg.scrape_interval.as_secs();
        if !(1..=3600).contains(&secs) {
            return Err(format!(
                "[green.cloud] scrape_interval_secs must be in [1, 3600], got {secs}"
            ));
        }
        if let Some(ref p) = cfg.default_provider
            && !matches!(p.as_str(), "aws" | "gcp" | "azure" | "scaleway")
        {
            return Err(format!(
                "[green.cloud] default_provider must be 'aws', 'gcp', 'azure' or \
                 'scaleway', got '{p}'"
            ));
        }
        if let Some(ref it) = cfg.default_instance_type
            && let Some(why) = unpriced_instance_suffix(it)
        {
            tracing::warn!(
                instance_type = %it,
                "[green.cloud] default_instance_type is not in the embedded \
                 SPECpower table; the provider default watts will be used{why}"
            );
        }
        if let Some(ref m) = cfg.cpu_metric
            && has_control_char(m)
        {
            return Err("[green.cloud] cpu_metric contains control characters".to_string());
        }
        Ok(())
    }

    /// Validate per-service entries in `[green.cloud.services]`: cardinality
    /// cap, name/control-char checks, watts ranges, instance type lookup.
    fn validate_cloud_services(cfg: &CloudEnergyConfig) -> Result<(), String> {
        const MAX_CLOUD_SERVICES: usize = 256;
        if cfg.services.len() > MAX_CLOUD_SERVICES {
            return Err(format!(
                "[green.cloud.services] has {} entries; maximum is {MAX_CLOUD_SERVICES}",
                cfg.services.len()
            ));
        }
        for (service, svc_cfg) in &cfg.services {
            Self::validate_cloud_service_name(service)?;
            Self::validate_cloud_service_cpu_query(service, svc_cfg)?;
            match svc_cfg {
                ServiceCloudConfig::ManualWatts {
                    idle_watts,
                    max_watts,
                    ..
                } => Self::validate_manual_watts(service, *idle_watts, *max_watts)?,
                ServiceCloudConfig::InstanceType {
                    provider,
                    instance_type,
                    ..
                } => Self::validate_instance_type_variant(
                    service,
                    provider.as_deref(),
                    instance_type,
                )?,
            }
        }
        warn_mixed_query_envelopes(cfg);
        Ok(())
    }

    /// Shape + control-char check on a cloud service name.
    fn validate_cloud_service_name(service: &str) -> Result<(), String> {
        if service.is_empty() || service.len() > 256 {
            return Err(format!(
                "[green.cloud.services] service name '{service}' must be 1-256 chars"
            ));
        }
        if has_control_char(service) {
            return Err(format!(
                "[green.cloud.services] service name '{service}' contains control characters"
            ));
        }
        Ok(())
    }

    /// Reject control characters in a service's optional per-service
    /// `cpu_query` override (log-injection / Prometheus-label-injection
    /// guard).
    fn validate_cloud_service_cpu_query(
        service: &str,
        svc_cfg: &ServiceCloudConfig,
    ) -> Result<(), String> {
        let Some(q) = svc_cfg.cpu_query() else {
            return Ok(());
        };
        if has_control_char(q) {
            return Err(format!(
                "[green.cloud.services.{service}] cpu_query contains control characters"
            ));
        }
        Ok(())
    }

    /// Validate a [`ServiceCloudConfig::ManualWatts`] arm: both values
    /// finite and non-negative, and `max_watts >= idle_watts`.
    fn validate_manual_watts(service: &str, idle_watts: f64, max_watts: f64) -> Result<(), String> {
        if !idle_watts.is_finite() || idle_watts < 0.0 {
            return Err(format!(
                "[green.cloud.services.{service}] idle_watts must be finite and >= 0, \
                 got {idle_watts}"
            ));
        }
        if !max_watts.is_finite() || max_watts < 0.0 {
            return Err(format!(
                "[green.cloud.services.{service}] max_watts must be finite and >= 0, \
                 got {max_watts}"
            ));
        }
        if max_watts < idle_watts {
            return Err(format!(
                "[green.cloud.services.{service}] max_watts ({max_watts}) must be \
                 >= idle_watts ({idle_watts})"
            ));
        }
        Ok(())
    }

    /// Validate a [`ServiceCloudConfig::InstanceType`] arm: provider
    /// allow-list, control-char rejection on `instance_type`, and a
    /// soft warning when the type is not in the embedded `SPECpower`
    /// table (not an error, the provider default is used instead).
    fn validate_instance_type_variant(
        service: &str,
        provider: Option<&str>,
        instance_type: &str,
    ) -> Result<(), String> {
        if let Some(p) = provider
            && !matches!(p, "aws" | "gcp" | "azure" | "scaleway")
        {
            return Err(format!(
                "[green.cloud.services.{service}] provider must be 'aws', 'gcp', \
                 'azure' or 'scaleway', got '{p}'"
            ));
        }
        if has_control_char(instance_type) {
            return Err(format!(
                "[green.cloud.services.{service}] instance_type contains control characters"
            ));
        }
        if !instance_type.is_empty()
            && let Some(why) = unpriced_instance_suffix(instance_type)
        {
            tracing::warn!(
                service = %service,
                instance_type = %instance_type,
                "[green.cloud.services] instance_type is not in the embedded \
                 SPECpower table; provider default watts will be used{why}"
            );
        }
        Ok(())
    }
}
