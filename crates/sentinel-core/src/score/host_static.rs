//! Declared database and broker hosts, the typed `[green.database_static]`
//! and `[green.broker_static]` sections.
//!
//! A section declares where the hosts run (`placement`): on hosts of their
//! own, or on an application node a cloud CPU query already reads. Only a
//! dedicated declaration with `nodes` and `instance_type` gives a ceiling,
//! which the daemon bills for either subsystem through
//! [`crate::score::broker_static::StaticBrokerState`]. The model and its
//! bounds are in `docs/design/05-GREENOPS-AND-CARBON.md` and
//! `docs/LIMITATIONS.md`.

use serde::{Deserialize, Serialize};

use crate::score::broker_static::StaticBrokerConfig;
use crate::score::cloud_energy::table::{InstanceClass, resolve_instance_class};

/// Where the declared hosts run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostPlacement {
    /// Hosts of their own, read by no application energy source.
    Dedicated,
    /// One application node, read by a `[green.cloud]` CPU query.
    Shared,
}

impl HostPlacement {
    /// The TOML spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dedicated => "dedicated",
            Self::Shared => "shared",
        }
    }
}

/// The two host workloads that emit no spans of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subsystem {
    Database,
    Broker,
}

impl Subsystem {
    /// The declared-hosts TOML section.
    #[must_use]
    pub const fn section(self) -> &'static str {
        match self {
            Self::Database => "[green.database_static]",
            Self::Broker => "[green.broker_static]",
        }
    }

    /// The Alumet workload TOML section.
    #[must_use]
    pub const fn alumet_section(self) -> &'static str {
        match self {
            Self::Database => "[green.alumet.database]",
            Self::Broker => "[green.alumet.broker]",
        }
    }

    /// The workload, as a noun for messages.
    #[must_use]
    pub const fn noun(self) -> &'static str {
        match self {
            Self::Database => "database",
            Self::Broker => "broker",
        }
    }

    /// The waste model tag of a window the declared ceiling billed.
    #[must_use]
    pub const fn declared_tag(self) -> &'static str {
        match self {
            Self::Database => crate::report::DB_WASTE_MODEL_SPECPOWER,
            Self::Broker => crate::report::BROKER_WASTE_MODEL_SPECPOWER,
        }
    }
}

/// A validated declared-hosts section. Strings are trimmed at load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticHostsConfig {
    pub placement: HostPlacement,
    /// Provisioned hosts, readers and standbys included.
    pub nodes: Option<u32>,
    /// An embedded row or a managed class that resolves to one.
    pub instance_type: Option<String>,
    /// Declared region. The effective one is
    /// [`crate::config::GreenConfig::host_region`].
    pub region: Option<String>,
    /// CPU of the declared hosts as a percent.
    pub cpu_query: Option<String>,
    /// `[green.cloud.services]` entries whose node a shared host runs on.
    pub shared_with: Vec<String>,
    /// `[[...hosts]]` groups of a dedicated declaration, each with its own
    /// `nodes`, `instance_type` and `cpu_query`. Empty when the section
    /// sets those keys at its top level instead.
    pub hosts: Vec<HostGroup>,
}

/// One group of identical hosts in a dedicated declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostGroup {
    pub nodes: Option<u32>,
    pub instance_type: Option<String>,
    pub cpu_query: Option<String>,
}

impl StaticHostsConfig {
    /// The declaration as one single-group declaration per host group, so
    /// each group bills, probes and falls back on its own. A section
    /// without `hosts` is its own single group.
    #[must_use]
    pub fn groups(&self) -> Vec<Self> {
        if self.hosts.is_empty() {
            return vec![self.clone()];
        }
        self.hosts
            .iter()
            .map(|group| Self {
                nodes: group.nodes,
                instance_type: group.instance_type.clone(),
                cpu_query: group.cpu_query.clone(),
                hosts: Vec::new(),
                ..self.clone()
            })
            .collect()
    }

    /// Idle and maximum watts of one host, `None` without an instance type
    /// or when the type resolves to no row.
    #[must_use]
    pub fn node_power(&self) -> Option<(f64, f64)> {
        match resolve_instance_class(self.instance_type.as_deref()?) {
            InstanceClass::Known {
                idle_watts,
                max_watts,
            } => Some((idle_watts, max_watts)),
            InstanceClass::Unsupported(_) | InstanceClass::Unknown => None,
        }
    }

    /// The provisioned ceiling: a dedicated declaration with both `nodes`
    /// and `instance_type`. A shared one has none, its ceiling would bill
    /// the whole application node.
    #[must_use]
    pub fn ceiling(&self) -> Option<StaticBrokerConfig> {
        if self.placement != HostPlacement::Dedicated {
            return None;
        }
        Some(StaticBrokerConfig {
            nodes: self.nodes?,
            // The original string: the resolver prices managed names.
            instance_type: self.instance_type.clone()?,
            provider: "generic".to_string(),
            region: self.region.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dedicated(nodes: u32, instance_type: &str) -> StaticHostsConfig {
        StaticHostsConfig {
            placement: HostPlacement::Dedicated,
            nodes: Some(nodes),
            instance_type: Some(instance_type.to_string()),
            region: Some("eu-west-3".to_string()),
            cpu_query: None,
            shared_with: Vec::new(),
            hosts: Vec::new(),
        }
    }

    #[test]
    fn shared_placement_has_no_ceiling() {
        let cfg = StaticHostsConfig {
            placement: HostPlacement::Shared,
            nodes: Some(1),
            ..dedicated(1, "m5.large")
        };
        assert_eq!(cfg.ceiling(), None);
    }

    #[test]
    fn ceiling_bills_nodes_times_the_resolved_maximum() {
        let cfg = dedicated(3, "m5.large");
        let ceiling = cfg.ceiling().expect("dedicated with nodes and type");
        let (_, max_watts) = cfg.node_power().expect("m5.large is a row");
        assert!((ceiling.cluster_watts() - 3.0 * max_watts).abs() < 1e-9);
        assert_eq!(ceiling.instance_type, "m5.large");
        assert_eq!(ceiling.region.as_deref(), Some("eu-west-3"));
    }

    #[test]
    fn dedicated_without_hosts_has_no_ceiling() {
        let cfg = StaticHostsConfig {
            nodes: None,
            instance_type: None,
            ..dedicated(1, "m5.large")
        };
        assert_eq!(cfg.ceiling(), None);
        assert_eq!(cfg.node_power(), None);
    }

    #[test]
    fn node_power_resolves_a_managed_name_to_its_family_row() {
        let managed = dedicated(1, "kafka.m5.large").node_power();
        assert!(managed.is_some());
        assert_eq!(managed, dedicated(1, "m5.large").node_power());
        assert_eq!(dedicated(1, "kafka.zz9.large").node_power(), None);
    }

    #[test]
    fn node_power_resolves_a_managed_database_class() {
        assert!(dedicated(2, "db.r6i.large").node_power().is_some());
        assert_eq!(
            dedicated(2, "db.r6i.large").node_power(),
            dedicated(2, "r6i.large").node_power()
        );
    }

    #[test]
    fn groups_split_hosts_into_single_group_declarations() {
        let flat = dedicated(2, "m5.large");
        assert_eq!(flat.groups(), vec![flat.clone()]);

        let grouped = StaticHostsConfig {
            nodes: None,
            instance_type: None,
            hosts: vec![
                HostGroup {
                    nodes: Some(1),
                    instance_type: Some("db.t4g.2xlarge".to_string()),
                    cpu_query: Some("q1".to_string()),
                },
                HostGroup {
                    nodes: Some(2),
                    instance_type: Some("db.t4g.small".to_string()),
                    cpu_query: None,
                },
            ],
            ..dedicated(1, "m5.large")
        };
        let groups = grouped.groups();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].nodes, Some(1));
        assert_eq!(groups[0].cpu_query.as_deref(), Some("q1"));
        assert_eq!(groups[1].instance_type.as_deref(), Some("db.t4g.small"));
        assert!(groups.iter().all(|g| g.hosts.is_empty()));
        assert!(
            groups
                .iter()
                .all(|g| g.region.as_deref() == Some("eu-west-3"))
        );
        assert!(groups[1].ceiling().is_some());
    }

    #[test]
    fn placement_spells_as_in_toml() {
        assert_eq!(HostPlacement::Dedicated.as_str(), "dedicated");
        assert_eq!(HostPlacement::Shared.as_str(), "shared");
    }
}
