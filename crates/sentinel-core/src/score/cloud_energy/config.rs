//! User-facing configuration for cloud-native energy estimation.

use std::borrow::Cow;
use std::collections::HashMap;
use std::time::Duration;

use super::table;

/// Default `PromQL` queries per cloud provider when no custom
/// `cpu_metric` or `cpu_query` is configured.
pub(crate) const DEFAULT_CPU_QUERY_AWS: &str = "aws_ec2_cpuutilization_average";
pub(crate) const DEFAULT_CPU_QUERY_GCP: &str =
    "stackdriver_gce_instance_compute_googleapis_com_instance_cpu_utilization";
pub(crate) const DEFAULT_CPU_QUERY_AZURE: &str =
    "azure_compute_virtualmachines_percentage_cpu_average";
pub(crate) const DEFAULT_CPU_QUERY_GENERIC: &str =
    "100 - (avg by(instance)(rate(node_cpu_seconds_total{mode=\"idle\"}[5m])) * 100)";

/// Per-service cloud energy configuration.
///
/// A service can be configured either by instance type (looked up in
/// the embedded `SPECpower` table) or by explicit idle/max watts
/// (for on-premise hardware not in the table).
#[derive(Debug, Clone)]
pub enum ServiceCloudConfig {
    /// Cloud provider instance with CPU% scraped from Prometheus.
    InstanceType {
        /// Cloud provider override for this service. When `None`, the
        /// top-level `default_provider` is used for table lookup fallback.
        provider: Option<String>,
        /// Instance type string (e.g. `"c5.4xlarge"`). Looked up in the
        /// embedded `SPECpower` table.
        instance_type: String,
        /// Optional custom `PromQL` query for this service's CPU%.
        /// When `None`, the top-level `cpu_metric` is used.
        cpu_query: Option<String>,
    },
    /// Manual watts specification (on-premise or custom hardware).
    ManualWatts {
        /// Power draw at near-zero CPU load (watts).
        idle_watts: f64,
        /// Power draw at 100% CPU utilization (watts).
        max_watts: f64,
        /// Optional custom `PromQL` query for this service's CPU%.
        cpu_query: Option<String>,
    },
}

/// Configuration for the cloud-native energy estimation subsystem.
///
/// Parsed from `[green.cloud]` in `.perf-sentinel.toml`. The subsystem
/// is only active when `prometheus_endpoint` is set.
#[derive(Clone)]
pub struct CloudEnergyConfig {
    /// Prometheus/VictoriaMetrics HTTP API endpoint
    /// (e.g. `"http://prometheus:9090"`). Must start with `http://`.
    pub prometheus_endpoint: String,
    /// How often to scrape CPU metrics. Default 15 s, clamped to
    /// `[1, 3600]` at config load.
    pub scrape_interval: Duration,
    /// Default cloud provider for services that don't specify one.
    /// Used as the `SPECpower` table fallback key. One of `"aws"`,
    /// `"gcp"`, `"azure"` or `None` for generic.
    pub default_provider: Option<String>,
    /// Default instance type for services that specify neither
    /// `instance_type` nor manual watts.
    pub default_instance_type: Option<String>,
    /// Default `PromQL` metric name / query template for CPU%.
    /// Used when a service has no `cpu_query` override.
    pub cpu_metric: Option<String>,
    /// Per-service configuration mapping service name to either an
    /// instance type lookup or manual watts override.
    pub services: HashMap<String, ServiceCloudConfig>,
    /// Optional auth header in curl format (`"Name: Value"`) attached
    /// to every Prometheus request. Required for Grafana Cloud, Grafana
    /// Mimir or any ingress that enforces bearer/basic auth. Stored as
    /// plain `String` (not `secrecy::SecretString`) to avoid adding a
    /// dependency. The manual `Debug` impl below redacts this field so
    /// `tracing::debug!(?config)` never leaks the credential. Resolved
    /// via the `PERF_SENTINEL_CLOUD_AUTH_HEADER` environment variable
    /// with fallback to this field. The env var wins when both are set.
    pub auth_header: Option<String>,
}

// Manual Debug impl to redact the auth header (potentially a secret).
impl std::fmt::Debug for CloudEnergyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudEnergyConfig")
            .field("prometheus_endpoint", &self.prometheus_endpoint)
            .field("scrape_interval", &self.scrape_interval)
            .field("default_provider", &self.default_provider)
            .field("default_instance_type", &self.default_instance_type)
            .field("cpu_metric", &self.cpu_metric)
            .field("services", &self.services)
            .field(
                "auth_header",
                &self.auth_header.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

impl ServiceCloudConfig {
    /// Returns the custom `PromQL` query, if any.
    #[must_use]
    pub fn cpu_query(&self) -> Option<&str> {
        match self {
            Self::InstanceType { cpu_query, .. } | Self::ManualWatts { cpu_query, .. } => {
                cpu_query.as_deref()
            }
        }
    }
}

impl CloudEnergyConfig {
    /// Resolve `(idle_watts, max_watts)` for a service config entry.
    ///
    /// For `InstanceType` entries, falls back to `default_instance_type`
    /// when the per-service instance type is empty, then to the provider
    /// default.
    #[must_use]
    pub fn service_power(&self, svc: &ServiceCloudConfig) -> (f64, f64) {
        match svc {
            ServiceCloudConfig::ManualWatts {
                idle_watts,
                max_watts,
                ..
            } => (*idle_watts, *max_watts),
            ServiceCloudConfig::InstanceType {
                provider,
                instance_type,
                ..
            } => {
                let provider_key = provider
                    .as_deref()
                    .or(self.default_provider.as_deref())
                    .unwrap_or("generic");
                let effective_type = if instance_type.is_empty() {
                    self.default_instance_type.as_deref().unwrap_or("")
                } else {
                    instance_type.as_str()
                };
                table::lookup_instance_power(effective_type, provider_key)
            }
        }
    }

    /// Resolve the `PromQL` query for a service, trimmed of leading and
    /// trailing whitespace.
    ///
    /// `PromQL` ignores that whitespace, so the trimmed form fetches the
    /// same series and serves as a stable key for comparing queries.
    /// Returns a `Cow` to avoid allocating when the query is a
    /// `&'static str` default or a borrowed reference from config.
    #[must_use]
    pub fn service_cpu_query<'a>(&'a self, svc: &'a ServiceCloudConfig) -> Cow<'a, str> {
        // 1. Per-service custom query
        if let Some(q) = svc.cpu_query() {
            return Cow::Borrowed(q.trim());
        }
        // 2. Top-level cpu_metric override
        if let Some(ref m) = self.cpu_metric {
            return Cow::Borrowed(m.trim());
        }
        // 3. Default per provider
        let provider = match svc {
            ServiceCloudConfig::InstanceType { provider, .. } => provider.as_deref(),
            ServiceCloudConfig::ManualWatts { .. } => None,
        };
        let provider = provider
            .or(self.default_provider.as_deref())
            .unwrap_or("generic");
        match provider {
            "aws" => Cow::Borrowed(DEFAULT_CPU_QUERY_AWS),
            "gcp" => Cow::Borrowed(DEFAULT_CPU_QUERY_GCP),
            "azure" => Cow::Borrowed(DEFAULT_CPU_QUERY_AZURE),
            _ => Cow::Borrowed(DEFAULT_CPU_QUERY_GENERIC),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> CloudEnergyConfig {
        CloudEnergyConfig {
            prometheus_endpoint: "http://prometheus:9090".to_string(),
            scrape_interval: Duration::from_secs(15),
            default_provider: Some("aws".to_string()),
            default_instance_type: Some("c5.xlarge".to_string()),
            cpu_metric: None,
            services: HashMap::new(),
            auth_header: Some("Authorization: Bearer super-secret-do-not-log".to_string()),
        }
    }

    #[test]
    fn debug_impl_redacts_auth_header() {
        // Guards against a `#[derive(Debug)]` on the struct, which
        // would print the credential.
        let cfg = sample_config();
        crate::test_helpers::assert_debug_redacts_secret!(&cfg, "super-secret-do-not-log");
    }

    #[test]
    fn debug_impl_preserves_non_secret_fields() {
        let cfg = sample_config();
        let dbg = format!("{cfg:?}");
        assert!(dbg.contains("prometheus_endpoint"));
        assert!(dbg.contains("http://prometheus:9090"));
        assert!(dbg.contains("c5.xlarge"));
        assert!(dbg.contains("aws"));
    }

    // ------------------------------------------------------------------
    // service_cpu_query
    // ------------------------------------------------------------------

    #[test]
    fn custom_cpu_query_takes_precedence() {
        let svc = ServiceCloudConfig::InstanceType {
            provider: Some("aws".into()),
            instance_type: "m5.large".into(),
            cpu_query: Some("custom_metric".into()),
        };
        let cfg = sample_config();
        assert_eq!(cfg.service_cpu_query(&svc), "custom_metric");
    }

    #[test]
    fn top_level_cpu_metric_used_when_no_per_service_query() {
        let svc = ServiceCloudConfig::InstanceType {
            provider: Some("aws".into()),
            instance_type: "m5.large".into(),
            cpu_query: None,
        };
        let mut cfg = sample_config();
        cfg.cpu_metric = Some("global_cpu_metric".into());
        assert_eq!(cfg.service_cpu_query(&svc), "global_cpu_metric");
    }

    #[test]
    fn provider_default_used_as_fallback() {
        let svc = ServiceCloudConfig::InstanceType {
            provider: Some("gcp".into()),
            instance_type: "n2-standard-2".into(),
            cpu_query: None,
        };
        let cfg = sample_config();
        assert_eq!(cfg.service_cpu_query(&svc), DEFAULT_CPU_QUERY_GCP);
    }

    // ------------------------------------------------------------------
    // service_power
    // ------------------------------------------------------------------

    #[test]
    fn manual_watts_used_directly() {
        let svc = ServiceCloudConfig::ManualWatts {
            idle_watts: 45.0,
            max_watts: 120.0,
            cpu_query: None,
        };
        let cfg = sample_config();
        let (idle, max) = cfg.service_power(&svc);
        assert!((idle - 45.0).abs() < 1e-10);
        assert!((max - 120.0).abs() < 1e-10);
    }

    #[test]
    fn instance_type_looked_up_in_table() {
        let svc = ServiceCloudConfig::InstanceType {
            provider: Some("aws".into()),
            instance_type: "c5.4xlarge".into(),
            cpu_query: None,
        };
        let cfg = sample_config();
        let (idle, max) = cfg.service_power(&svc);
        // c5.4xlarge (16 vCPU, Cascade Lake) at CCF 2026-04-24:
        // 16 * 0.690 = 11.0 idle, 16 * 4.063 = 65.0 max.
        assert!((idle - 11.0).abs() < 0.01);
        assert!((max - 65.0).abs() < 0.1);
    }

    // ------------------------------------------------------------------
    // service_cpu_query: exhaustive provider-branch coverage
    // ------------------------------------------------------------------

    #[test]
    fn azure_provider_uses_azure_default_query() {
        let svc = ServiceCloudConfig::InstanceType {
            provider: Some("azure".into()),
            instance_type: "Standard_D4s_v3".into(),
            cpu_query: None,
        };
        let cfg = sample_config();
        assert_eq!(cfg.service_cpu_query(&svc), DEFAULT_CPU_QUERY_AZURE);
    }

    #[test]
    fn aws_provider_uses_aws_default_query() {
        let svc = ServiceCloudConfig::InstanceType {
            provider: Some("aws".into()),
            instance_type: "m5.large".into(),
            cpu_query: None,
        };
        let cfg = sample_config();
        assert_eq!(cfg.service_cpu_query(&svc), DEFAULT_CPU_QUERY_AWS);
    }

    #[test]
    fn unknown_provider_falls_back_to_generic_query() {
        let svc = ServiceCloudConfig::InstanceType {
            provider: Some("hetzner".into()),
            instance_type: "cx21".into(),
            cpu_query: None,
        };
        let cfg = sample_config();
        assert_eq!(cfg.service_cpu_query(&svc), DEFAULT_CPU_QUERY_GENERIC);
    }

    #[test]
    fn manual_watts_variant_uses_default_provider_for_query() {
        // ManualWatts has no provider field, so service_cpu_query must
        // fall back to cfg.default_provider, exercising its second
        // match arm.
        let svc = ServiceCloudConfig::ManualWatts {
            idle_watts: 10.0,
            max_watts: 100.0,
            cpu_query: None,
        };
        let mut cfg = sample_config();
        cfg.default_provider = Some("azure".into());
        assert_eq!(cfg.service_cpu_query(&svc), DEFAULT_CPU_QUERY_AZURE);
    }

    #[test]
    fn manual_watts_variant_no_default_provider_uses_generic() {
        let svc = ServiceCloudConfig::ManualWatts {
            idle_watts: 10.0,
            max_watts: 100.0,
            cpu_query: None,
        };
        let mut cfg = sample_config();
        cfg.default_provider = None;
        assert_eq!(cfg.service_cpu_query(&svc), DEFAULT_CPU_QUERY_GENERIC);
    }

    #[test]
    fn service_power_falls_back_to_default_instance_type() {
        // An empty svc_cfg.instance_type falls back to cfg.default_instance_type.
        let svc = ServiceCloudConfig::InstanceType {
            provider: Some("aws".into()),
            instance_type: String::new(), // empty, takes the fallback path
            cpu_query: None,
        };
        let mut cfg = sample_config();
        cfg.default_instance_type = Some("c5.4xlarge".into());
        let (idle, max) = cfg.service_power(&svc);
        // c5.4xlarge per CCF 2026-04-24 Cascade Lake coefficient.
        assert!((idle - 11.0).abs() < 0.01);
        assert!((max - 65.0).abs() < 0.1);
    }

    #[test]
    fn padded_per_service_cpu_query_comes_back_trimmed() {
        let svc = ServiceCloudConfig::InstanceType {
            provider: Some("aws".into()),
            instance_type: "m5.large".into(),
            cpu_query: Some("  \n custom_metric{job=\"db\"}\t ".into()),
        };
        let cfg = sample_config();
        assert_eq!(cfg.service_cpu_query(&svc), "custom_metric{job=\"db\"}");
    }
}
