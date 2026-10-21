use super::*;

/// The published list must hold every type the lookup accepts, and
/// claim no type it does not. `refresh-instance-power.py` rewrites the
/// table from upstream coefficients, so a hand-maintained page would
/// go stale on the first refresh and quietly send operators to a
/// fallback average. Regenerate with
/// `python3 scripts/generate-instance-types-doc.py`.
#[test]
fn instance_types_doc_matches_the_table() {
    fn rows(doc: &str) -> HashMap<&str, (f64, f64)> {
        doc.lines()
            .filter_map(|line| {
                let mut columns = line.split('|').map(str::trim);
                columns.next()?;
                let name = columns.next()?.strip_prefix('`')?.strip_suffix('`')?;
                let idle = columns.next()?.parse().ok()?;
                let max = columns.next()?.parse().ok()?;
                Some((name, (idle, max)))
            })
            .collect()
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    for relative in ["INSTANCE-TYPES.md", "FR/INSTANCE-TYPES-FR.md"] {
        let doc = std::fs::read_to_string(root.join(relative)).expect("read instance docs");
        let listed = rows(&doc);
        assert_eq!(listed.len(), INSTANCE_POWER.len(), "{relative}");
        for (name, expected) in INSTANCE_POWER.iter() {
            assert_eq!(listed.get(name), Some(expected), "{relative}: {name}");
        }
        assert!(doc.contains(SPECPOWER_VINTAGE), "{relative}: vintage");
        let bare_metal = doc.find("## Bare metal").expect("bare-metal section");
        let xeon = doc.find("`xeon-6780e`").expect("bare-metal row");
        assert!(xeon > bare_metal, "{relative}: bare-metal section");
    }
}

// ------------------------------------------------------------------
// lookup_instance_power
// ------------------------------------------------------------------

// Sizes within a family must scale linearly with vCPU count, per
// the `vCPU * coefficient` methodology. Relation-based so a data
// refresh does not break them. Exact values are reviewed on the
// refresh PR diff.
#[test]
fn known_aws_instance_scales_with_vcpu() {
    let (idle_2, max_2) = lookup_instance_power("m5.large", "aws");
    let (idle_4, max_4) = lookup_instance_power("m5.xlarge", "aws");
    assert!((idle_4 - 2.0 * idle_2).abs() < 0.1);
    assert!((max_4 - 2.0 * max_2).abs() < 0.2);
}

#[test]
fn known_gcp_instance_scales_with_vcpu() {
    let (idle_2, max_2) = lookup_instance_power("n2-standard-2", "gcp");
    let (idle_8, max_8) = lookup_instance_power("n2-standard-8", "gcp");
    assert!((idle_8 - 4.0 * idle_2).abs() < 0.2);
    assert!((max_8 - 4.0 * max_2).abs() < 0.4);
}

#[test]
fn known_azure_instance_scales_with_vcpu() {
    let (idle_2, max_2) = lookup_instance_power("Standard_D2s_v3", "azure");
    let (idle_8, max_8) = lookup_instance_power("Standard_D8s_v3", "azure");
    assert!((idle_8 - 4.0 * idle_2).abs() < 0.2);
    assert!((max_8 - 4.0 * max_2).abs() < 0.4);
}

#[test]
fn unknown_instance_falls_back_to_provider_default() {
    let fallback = lookup_instance_power("m999.future", "aws");
    let default = *PROVIDER_DEFAULTS.get("aws").expect("aws default");
    assert_eq!(fallback, default);
}

// The generated/manual split relies on disjoint keys: a HashMap
// collision would silently shadow a generated row with a stale
// manual one (the exact mistake the refresh-script watchlist
// invites when a family moves from manual to generated).
#[test]
fn generated_and_manual_instance_keys_are_disjoint() {
    assert_eq!(
        INSTANCE_POWER.len(),
        crate::score::cloud_energy::table_data::GENERATED_INSTANCE_ROWS.len()
            + MANUAL_INSTANCE_ROWS.len(),
        "a manual instance row shadows a generated one"
    );
}

// Cross-CSV sanity anchor: the same architecture (Cascade Lake, 2
// vCPU) must land in the same ballpark across provider CSVs. A
// mis-scaled or mis-parsed Azure CSV passes the positivity and
// linear-scaling tests, but not this one.
#[test]
fn azure_scale_sane_vs_aws_same_architecture() {
    let (aws_idle, aws_max) = lookup_instance_power("m5.large", "aws");
    let (az_idle, az_max) = lookup_instance_power("Standard_D2s_v4", "azure");
    assert!(
        az_idle > aws_idle * 0.5 && az_idle < aws_idle * 2.0,
        "Azure Cascade Lake idle {az_idle} out of 2x band vs AWS {aws_idle}"
    );
    assert!(
        az_max > aws_max * 0.5 && az_max < aws_max * 2.0,
        "Azure Cascade Lake max {az_max} out of 2x band vs AWS {aws_max}"
    );
}

/// Scaleway rows are derived from the catalog's exact CPU rather
/// than from a published wattage, so this test pins the invariants
/// that keep them correct. The same silicon must land on the same
/// per-vCPU figure as the provider whose coefficient it borrows, and
/// the excluded categories must stay out.
#[test]
fn scaleway_rows_agree_with_the_architecture_they_borrow() {
    // Turin uses the GCP c4d SPECpower compute, same EPYC 9005
    // generation, so 2 vCPU must price identically on both.
    assert_eq!(
        lookup_instance_power("COMPUTE3-X2C-4G", "scaleway"),
        lookup_instance_power("c4d-standard-2", "gcp"),
        "Turin rows must match the c4d reference they are derived from"
    );
    // POP2 is EPYC 3rd Gen (Milan) from the AWS CSV, the same
    // coefficient m6a uses: 2 vCPU against m6a.large's 2 vCPU.
    assert_eq!(
        lookup_instance_power("POP2-2C-8G", "scaleway"),
        lookup_instance_power("m6a.large", "aws"),
        "POP2 must match the Milan coefficient it borrows"
    );
    // The memory-optimized range carries the DRAM premium, so it
    // must draw strictly more than the compute range at equal vCPU.
    let (mem_idle, mem_max) = lookup_instance_power("MEMORY3-X2C-16G", "scaleway");
    let (cpu_idle, cpu_max) = lookup_instance_power("COMPUTE3-X2C-4G", "scaleway");
    assert!(
        mem_idle > cpu_idle && mem_max > cpu_max,
        "MEMORY3 carries 8 GiB/vCPU, so it must exceed COMPUTE3: {mem_idle}/{mem_max} vs {cpu_idle}/{cpu_max}"
    );
    // Shared-vCPU, GPU and unmapped-architecture ranges stay out:
    // each would need an assumption the model does not make.
    for excluded in ["DEV1-S", "PLAY2-PICO", "H100-1-80G", "STANDARD2-X2C-8G"] {
        assert!(
            !is_known_instance_type(excluded),
            "{excluded} must stay out of the table"
        );
    }
}

#[test]
fn modern_architecture_keys_present() {
    for key in [
        "m7i.large",
        "c7a.large",
        "r7a.large",
        "m6a.xlarge",
        "c7g.large",
        "m8g.large",
        "m8a.large",
        "c8a.large",
        "m8i.large",
        "c8i.large",
        "c4-standard-4",
        "c4d-standard-8",
        "c4a-standard-2",
        "t2a-standard-2",
        "Standard_D2s_v6",
        "Standard_D2ps_v6",
        "xeon-6780e",
        "m6g.large",
        "r6g.large",
        "t4g.micro",
        "r7g.large",
        "r8g.large",
        "m7g.medium",
        "n1-standard-4",
        "Standard_E4as_v5",
        "Standard_B2ms",
    ] {
        assert!(is_known_instance_type(key), "missing modern entry: {key}");
    }
}

#[test]
fn turin_overrides_to_genoa_proxy() {
    // EPYC 5th Gen Turin (m8a/c8a) is proxied to EPYC 4th Gen Genoa
    // (m7a/c7a) because the CCF 2026-04-24 Turin row is anomalously
    // high. If this test fails, re-evaluate the override against the
    // current CCF snapshot before silently aligning to a new value.
    // See docs/LIMITATIONS.md section "EPYC 5th Gen Turin" for the
    // rationale and the revalidation procedure.
    let sizes = [
        "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "16xlarge",
    ];
    for size in sizes {
        for (turin, genoa) in [("m8a", "m7a"), ("c8a", "c7a")] {
            let turin_key = format!("{turin}.{size}");
            let genoa_key = format!("{genoa}.{size}");
            assert_eq!(
                lookup_instance_power(&turin_key, "aws"),
                lookup_instance_power(&genoa_key, "aws"),
                "{turin_key} (Turin) must alias to {genoa_key} (Genoa) until CCF correction"
            );
        }
    }
}

#[test]
fn m_series_does_not_carry_dram_premium() {
    // m5 and c5 share the bare Cascade Lake coefficient (CCF does
    // not differentiate general-purpose vs compute). If a refactor
    // applied the DRAM premium to general-purpose families, m5
    // would drift away from c5 and this equality would break.
    assert_eq!(
        lookup_instance_power("m5.large", "aws"),
        lookup_instance_power("c5.large", "aws"),
        "m5.large must stay on the bare coefficient, like c5.large"
    );
}

#[test]
fn r_series_includes_dram_premium_over_general_purpose() {
    // r5.large (2 vCPU memory-optimized, Cascade Lake) should carry
    // an additive DRAM premium over m5.large (2 vCPU general-purpose,
    // same Cascade Lake): 2 vCPU * 8 GB/vCPU * 0.02 W/GB = 0.32 idle,
    // 2 vCPU * 8 GB/vCPU * 0.05 W/GB = 0.80 max. If this test fails,
    // re-check that DRAM_PREMIUM_W_PER_GB_{IDLE,MAX} and the inline
    // r-series values stay in sync.
    let (m5_idle, m5_max) = lookup_instance_power("m5.large", "aws");
    let (r5_idle, r5_max) = lookup_instance_power("r5.large", "aws");
    assert!(
        (r5_idle - m5_idle - 0.32).abs() < 0.05,
        "DRAM idle uplift drift: r5 {r5_idle} - m5 {m5_idle} expected ~0.32"
    );
    assert!(
        (r5_max - m5_max - 0.80).abs() < 0.05,
        "DRAM max uplift drift: r5 {r5_max} - m5 {m5_max} expected ~0.80"
    );
}

#[test]
fn sierra_forest_entries_are_chip_level_not_vcpu_level() {
    // Sierra Forest entries are 1-chip system-level watts, not vCPU-scaled.
    // A vCPU-scaled value would never exceed ~6 W idle for a 144-thread
    // entry, so floor at 50 W idle catches accidental rescaling errors.
    let (idle, _) = lookup_instance_power("xeon-6780e", "generic");
    assert!(
        idle >= 50.0,
        "xeon-6780e must be system-level (>=50W idle), got {idle}"
    );
}

#[test]
fn unknown_provider_falls_back_to_generic() {
    let (idle, max) = lookup_instance_power("custom.instance", "onprem");
    assert!((idle - 3.0).abs() < 0.01);
    assert!((max - 20.0).abs() < 0.01);
}

#[test]
fn is_known_true_for_table_entry() {
    assert!(is_known_instance_type("c5.4xlarge"));
}

#[test]
fn is_known_false_for_missing_entry() {
    assert!(!is_known_instance_type("m99.jumbo"));
}

// ------------------------------------------------------------------
// interpolate_watts
// ------------------------------------------------------------------

#[test]
fn interpolate_at_zero_percent() {
    let w = interpolate_watts(2.0, 20.0, 0.0);
    assert!((w - 2.0).abs() < 1e-10);
}

#[test]
fn interpolate_at_fifty_percent() {
    let w = interpolate_watts(2.0, 20.0, 50.0);
    assert!((w - 11.0).abs() < 1e-10);
}

#[test]
fn interpolate_at_hundred_percent() {
    let w = interpolate_watts(2.0, 20.0, 100.0);
    assert!((w - 20.0).abs() < 1e-10);
}

#[test]
fn interpolate_clamps_below_zero() {
    let w = interpolate_watts(2.0, 20.0, -10.0);
    assert!((w - 2.0).abs() < 1e-10, "should clamp to idle");
}

#[test]
fn interpolate_clamps_above_hundred() {
    let w = interpolate_watts(2.0, 20.0, 150.0);
    assert!((w - 20.0).abs() < 1e-10, "should clamp to max");
}

#[test]
fn interpolate_nan_returns_idle() {
    let w = interpolate_watts(2.0, 20.0, f64::NAN);
    assert!((w - 2.0).abs() < 1e-10, "NaN input should return idle");
}

#[test]
fn interpolate_infinity_returns_idle() {
    let w = interpolate_watts(2.0, 20.0, f64::INFINITY);
    assert!((w - 2.0).abs() < 1e-10, "Inf input should return idle");
}

// ------------------------------------------------------------------
// compute_cloud_energy_per_op_kwh
// ------------------------------------------------------------------

#[test]
fn basic_energy_computation() {
    // 10 W for 15 seconds, 100 ops.
    // kWh = 10/1000 * 15/3600 = 0.0000416667
    // per_op = 0.0000416667 / 100 = 4.16667e-7
    let result = compute_cloud_energy_per_op_kwh(10.0, 15.0, 100);
    assert!(result.is_some());
    let per_op = result.unwrap();
    let expected = (10.0 / 1000.0) * (15.0 / 3600.0) / 100.0;
    assert!((per_op - expected).abs() < 1e-15);
}

#[test]
fn zero_ops_returns_none() {
    assert!(compute_cloud_energy_per_op_kwh(10.0, 15.0, 0).is_none());
}

#[test]
fn negative_watts_returns_none() {
    assert!(compute_cloud_energy_per_op_kwh(-1.0, 15.0, 100).is_none());
}

#[test]
fn nan_watts_returns_none() {
    assert!(compute_cloud_energy_per_op_kwh(f64::NAN, 15.0, 100).is_none());
}

#[test]
fn infinite_watts_returns_none() {
    assert!(compute_cloud_energy_per_op_kwh(f64::INFINITY, 15.0, 100).is_none());
}

// ------------------------------------------------------------------
// Table integrity
// ------------------------------------------------------------------

#[test]
fn all_entries_have_positive_values() {
    for (name, &(idle, max)) in INSTANCE_POWER.iter() {
        assert!(idle > 0.0, "{name}: idle must be positive, got {idle}");
        assert!(max > 0.0, "{name}: max must be positive, got {max}");
        assert!(max >= idle, "{name}: max ({max}) must be >= idle ({idle})");
    }
}

#[test]
fn table_has_expected_entry_count() {
    // CCF 2026-04-24 vCPU * coefficient across AWS/GCP/Azure, plus a
    // handful of direct-SPECpower entries where the provider CSV has
    // no row (Azure v6, GCP t2a/c4d, Cobalt 100), plus 1 Sierra Forest
    // CPU-named system-level row. Conservative floor that tolerates
    // minor entry pruning.
    assert!(
        INSTANCE_POWER.len() >= 300,
        "expected >= 300 entries, got {}",
        INSTANCE_POWER.len()
    );
}

// ------------------------------------------------------------------
// resolve_instance_class
// ------------------------------------------------------------------

fn row_class(name: &str) -> InstanceClass {
    let (idle_watts, max_watts) = *INSTANCE_POWER.get(name).expect("reference row");
    InstanceClass::Known {
        idle_watts,
        max_watts,
    }
}

#[test]
fn managed_and_modifier_names_resolve_to_their_vm_row() {
    for (alias, row) in [
        ("db.r6i.large", "r6i.large"),
        ("cache.r5.large", "r5.large"),
        ("kafka.m5.large", "m5.large"),
        ("express.m7g.large", "m7g.large"),
        ("mq.t3.micro", "t3.micro"),
        ("r5.large.search", "r5.large"),
        ("m5.large.elasticsearch", "m5.large"),
        ("db.m5d.large", "m5.large"),
        ("db.r6idn.large", "r6i.large"),
        ("c7gn.large", "c7g.large"),
        ("Standard_D4ds_v5", "Standard_D4s_v5"),
        ("Standard_E8ds_v5", "Standard_E8s_v5"),
        ("Standard_D4ads_v5", "Standard_D4as_v5"),
        ("Standard_D4pds_v6", "Standard_D4ps_v6"),
        ("db.m6g.large", "m6g.large"),
        ("db.m6gd.large", "m6g.large"),
        ("cache.r7g.large", "r7g.large"),
    ] {
        assert_eq!(resolve_instance_class(alias), row_class(row), "{alias}");
    }
}

#[test]
fn left_out_classes_carry_their_reason() {
    for (name, reason) in [
        ("db.serverless", "ACU"),
        ("db-f1-micro", "shared"),
        ("db-g1-small", "shared"),
        ("redis-highmem-medium", "machine series"),
    ] {
        match resolve_instance_class(name) {
            InstanceClass::Unsupported(text) => assert!(text.contains(reason), "{name}: {text}"),
            other => panic!("{name} must be unsupported, got {other:?}"),
        }
    }
}

#[test]
fn ungrounded_names_stay_unknown() {
    // z, e and i are never stripped.
    for name in ["m5zn.large", "db.x2iedn.large", "Standard_E80ids_v4", ""] {
        assert_eq!(
            resolve_instance_class(name),
            InstanceClass::Unknown,
            "{name}"
        );
    }
}

#[test]
fn exact_rows_are_never_rewritten() {
    for name in ["m5.large", "c7g.large", "Standard_D2ads_v6"] {
        assert_eq!(resolve_instance_class(name), row_class(name), "{name}");
    }
}

#[test]
fn lookup_keeps_the_provider_default_for_unresolved_names() {
    let default = *PROVIDER_DEFAULTS.get("aws").expect("aws default");
    assert_eq!(lookup_instance_power("db.serverless", "aws"), default);
    assert_eq!(lookup_instance_power("m5zn.large", "aws"), default);
    assert_eq!(
        lookup_instance_power("db.r6i.large", "aws"),
        lookup_instance_power("r6i.large", "aws")
    );
    assert!(is_known_instance_type("db.r6i.large"));
    assert!(!is_known_instance_type("db.serverless"));
}

// ------------------------------------------------------------------
// Row relations (hold whatever the refreshed coefficients are)
// ------------------------------------------------------------------

/// One rounding step of the generated rows, which round to 0.1 W.
const ROUNDING_STEP: f64 = 0.1 + 1e-9;

fn row(name: &str) -> (f64, f64) {
    *INSTANCE_POWER.get(name).expect("generated row")
}

fn assert_within_step(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() <= ROUNDING_STEP,
        "{what}: {actual} vs {expected}"
    );
}

#[test]
fn memory_optimized_graviton_adds_the_dram_premium_per_vcpu() {
    let (r_idle, r_max) = row("r6g.large");
    let (m_idle, m_max) = row("m6g.large");
    assert_within_step(r_idle - m_idle, 2.0 * 0.16, "idle premium");
    assert_within_step(r_max - m_max, 2.0 * 0.40, "max premium");
}

#[test]
fn graviton_medium_is_one_vcpu() {
    let (medium_idle, medium_max) = row("m6g.medium");
    let (large_idle, large_max) = row("m6g.large");
    assert_within_step(medium_idle, large_idle / 2.0, "idle");
    assert_within_step(medium_max, large_max / 2.0, "max");
}

#[test]
fn equal_vcpu_rows_of_one_platform_are_equal() {
    // t4g counts its sub-xlarge sizes at 2 vCPU like t3, and n1-highmem
    // sits below the 8 GiB per vCPU DRAM threshold.
    assert_eq!(row("t4g.nano"), row("t4g.large"));
    assert_eq!(row("n1-highmem-8"), row("n1-standard-8"));
}
