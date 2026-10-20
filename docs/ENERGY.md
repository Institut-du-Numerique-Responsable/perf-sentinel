# Understanding the energy and carbon figures

This page tells the whole energy story in plain language: what Perf Sentinel counts, what it measures, how a number of I/O operations becomes kilowatt-hours and grams of CO2, and what changes depending on which options you enable. It is a synthesis, not a reference. The formulas live in [METHODOLOGY.md](METHODOLOGY.md), the precision bounds in [LIMITATIONS.md](LIMITATIONS.md), and every configuration key in [CONFIGURATION.md](CONFIGURATION.md).

## The idea in one paragraph

Perf Sentinel reads distributed traces and counts every I/O operation an application performs: SQL queries, outbound HTTP calls and broker publishes. Its detectors flag the operations that did not need to happen, mainly N+1 loops and redundant repeated calls. The ratio between avoidable and total operations is the waste ratio, and it is the most robust number the tool produces because it does not depend on any energy model. Everything else in this page is about turning the operation counts into energy and carbon: the waste ratio tells you which share is wasted, the energy pipeline tells you how much that share weighs in kWh and gCO2.

## Where each number comes from

The carbon figures follow the Software Carbon Intensity model (SCI, standardized as ISO/IEC 21031:2024): carbon = energy x grid intensity, plus an embodied term for the hardware itself.

- **Energy (E)** starts as an estimate and becomes a measurement as you plug in backends. With nothing configured, every operation costs the emitting service a fixed `1e-7 kWh` (the proxy coefficient, model tag `io_proxy_v3`). Each backend below replaces that estimate with something closer to physical reality.
- **Grid intensity (I)** converts kWh into gCO2 for the region where the code runs. It starts from embedded annual national averages, can be modulated by 24-hour profiles, and becomes a live value when the Electricity Maps API is configured. The region itself comes from the `cloud.region` span attribute, from `[green.service_regions]`, or from `[green] default_region`, in that order.
- **Embodied carbon (M)** accounts for manufacturing the servers. It is a fixed per-request figure derived from public lifecycle assessments of rack servers (Boavizta and the Cloud Carbon Footprint methodology), configurable via `embodied_carbon_per_request_gco2`. Fixing an N+1 does not un-manufacture silicon, so the avoidable figures never include this term.
- **PUE** multiplies the energy to account for datacenter overhead (cooling, power distribution): 1.09 for GCP, 1.15 for AWS, 1.17 for Azure, 1.24 for OVHcloud, 1.375 for Scaleway, 1.5 for OUTSCALE (which publishes none) and for unknown infrastructure.

Every report states which model produced its numbers through the `energy_model` and `per_service_energy_model` tags, so a reader can always tell an estimate from a measurement.

## The fidelity ladder

Each row of this table replaces or refines the row above it, on the same perimeter: the hosts of the traced services. Every rung of the ladder, the I/O proxy included, prices the emitting service's own host for each operation it issues: CPU, memory and network stack on that host. The callee (database server, broker, downstream service) is never in it. Database and broker host energy enters a report only through a declared or measured source: `[green.alumet.database]`, `[green.alumet.broker]` or `[green.broker_static]`. SQL verb weights and HTTP tiers are relative weights that rank operation classes. Their ratios come from database-server benchmarks, but they bill no database host. You can stop at any rung, and the reports always state which rung you are on.

| You configure                         | What you get                                                                                                                                                                                                   | Model tag                    |
|---------------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|------------------------------|
| Nothing (default)                     | Fixed `1e-7 kWh` per operation for the emitting service, weighted by SQL verb (SELECT 0.5x, INSERT/UPDATE 1.5x, DELETE 1.2x) and HTTP payload size tiers. Directional, order-of-magnitude, carries a 2x uncertainty bracket.            | `io_proxy_v3` |
| `calibrate` from a measured power CSV | The proxy coefficient is rescaled per service from your own measured joules per operation, read on the service's own host and never on its database or broker. Still a model, but anchored to your hardware.                                                                       | `io_proxy_*+cal` |
| `[green.cloud]`                       | Cloud VM energy interpolated from CPU utilization and the public SPECpower database of server power curves, following the Cloud Carbon Footprint methodology.                                                  | `cloud_specpower`            |
| `[green.redfish]`                     | Power of the whole chassis, read from the server's BMC. The only backend that sees fans, disks and network cards, plus power supply losses when the BMC measures at the supply input. Physical servers only.   | `redfish_bmc`                |
| `[green.kepler]`                      | Kepler's RAPL readings per container or process. Ranked below the other RAPL backends: it splits only the active power, by CPU time, and its 0.10 rewrite has no independent evaluation yet, see the sources.  | `kepler_ebpf`                |
| `[green.scaphandre]`                  | CPU energy from Intel RAPL counters, attributed per process by Scaphandre.                                                                                                                                     | `scaphandre_rapl`            |
| `[green.alumet]`                      | CPU energy from RAPL, attributed per cgroup by Alumet. The recommended measured backend: same counters as Scaphandre, sampling characterized as less error-prone by its authors, container-shaped attribution. | `alumet_rapl`                |
| `[green.electricity_maps]`            | Does not change E. Replaces the annual grid intensity with the live value for your region, which is the largest lever on the gCO2 figures in regions with variable electricity mixes.                          | intensity source `real_time` |
| `[green.rte_eco2mix]`, `[green.neso]`, `[green.fingrid]`, `[green.energinet]`, `[green.ree_esios]`, `[green.open_electricity]`, `[green.mavir]`, `[green.em6]` | Does not change E. The free counterparts for France, Great Britain, Finland, Denmark, Spain, Australia, Hungary and New Zealand: live direct emissions published by the grid operator, lower than a lifecycle figure. Electricity Maps wins where both apply. | intensity source `real_time`, provider named on the region row |

When several backends cover the same service, the daemon keeps the highest-fidelity reading: `alumet_rapl` beats `scaphandre_rapl`, which beats `kepler_ebpf`, then `redfish_bmc`, then `cloud_specpower`, then the proxy. All measured backends are daemon-only (`watch`). Batch `analyze` always uses the proxy path.

**Each node is counted once.** The services that read one cloud CPU query returning a single series, or one Redfish chassis, split that node's energy by the operations each emitted in the interval, instead of each receiving the whole node. A service on the node that a higher-fidelity backend prices keeps its operations in that count, so its share is dropped rather than handed to its neighbours. Node-level sources must therefore list every traced service on the node: a traced service left out leaves its share to the listed ones. The cloud rules, mixed power figures and multi-series queries included, are in [CONFIGURATION.md](CONFIGURATION.md#greencloud-optional-opt-in).

One constraint applies to every RAPL-based rung: the hardware counters only see CPU and DRAM, which is roughly half to two thirds of what the server draws at the wall. Only Redfish also covers fans, disks and network cards, and power supply losses only when the BMC measures at the supply input.

## The database figure

Counting operations on the application side misses a structural point: the energy of an N+1 is mostly burned by the database that executes the N queries, and a database emits no spans, so it is invisible to the per-service attribution. The `[green.alumet.database]` declaration closes that gap with a simple rule of three: point Alumet at the database cgroup, and each scoring window multiplies the measured database energy by the SQL-only waste ratio.

```
database waste = measured DB energy x (avoidable SQL ops / total SQL ops)
```

The result is `green_summary.database_waste`, with a gCO2 conversion when you declare the database's region. And the figure exists even without Alumet: when no measurement is available (batch runs, managed databases, no `[green.alumet.database]`), it falls back to the application-side share of the SQL spans, the energy span scoring gave the services that issued them, with no database host energy in it. Its `model` tag says which path produced it (`alumet_rapl` = measured, `estimated` = application-side share). Measured is a lower bound (CPU energy only, no DRAM, no disk). Estimated is only as precise as the energy model behind the report (the I/O proxy and its 2x bracket in the common case). Both use a count-based ratio, so the figure stays informational. The measured variant is database host energy kept beside `energy_kwh` and `co2`. The estimated variant is a share already inside those totals (never add it on top). The disclosure publishes the figure as the separate `aggregate.database_waste` block, never added to a total. The full list of bounds is in [LIMITATIONS.md](LIMITATIONS.md#alumet-precision-bounds).

## The broker figure

A message broker poses the database's problem twice over: it burns the energy of an N+1 publish loop, it emits no span of its own, and it is very often managed, so there is no host to run an agent on. The same rule of three answers it, with the messaging-only ratio.

```
broker waste = broker energy x (avoidable publish ops / total publish ops)
```

The result is `green_summary.messaging_waste`. What differs from the database is the number of ways the broker energy can be obtained, and they do not have the same standing.

- `[green.alumet.broker]` measures the broker cgroup. Same rung as the database, same bound: Alumet's default attribution formula covers CPU only, so it under-counts.
- `[green.broker_static]` declares a provisioned cluster (`nodes` plus an instance type) and needs no agent, which suits a provisioned cluster with a typed instance class, such as a managed Kafka or MQ cluster. Serverless and capacity-unit brokers have no provisioned node and cannot be declared. It reads `E(n) = n x P_max` from the embedded SPECpower table. That table covers CPU and baseboard, so the result bounds the declared vCPUs at full load and not the cluster's wall power, and a storage-bound broker can draw more. It also does not move when the application starts batching, which is a real weakness for a figure meant to show remediation working.
- With neither declared, the figure is the application-side share of the publish spans, exactly like the database fallback: already inside the totals, with no broker host energy in it.

The `model` tag says which path produced it (`alumet_rapl`, `broker_specpower`, `estimated`), and the periodic disclosure keeps the three apart rather than folding a declaration into the measured sums. A measured or declared figure sits beside the totals, the estimate inside them, and none is ever added to a total. None of this relies on a joules-per-message coefficient. Broker power stops tracking throughput past roughly 20 % of capacity, so the marginal energy of one publish is not a constant. The three things that would determine it (cluster utilization, replication factor, topology) are all invisible from a producer span. The bounds are in [LIMITATIONS.md](LIMITATIONS.md).

## What the numbers are not

The tool is a directional waste counter with increasingly good energy anchoring, not a wattmeter and not a certified carbon inventory. The proxy path carries a 2x multiplicative bracket. Idle power depends on the rung: the proxy, Kepler and Alumet's usual attribution formula leave it unattributed, while cloud SPECpower and Redfish include the node's idle power, split between the services reading that node. The count-based ratios behind `co2.avoidable`, both disclosure waste tiers and the database and broker figures treat a cheap indexed SELECT and a heavy write the same, which academic measurements show can differ by tens of percent in power terms, and a slow query weighs one operation like a fast one. All of this is quantified, with the reasoning, in [LIMITATIONS.md](LIMITATIONS.md).

## Sources

What each external source contributes to the numbers above:

- **Green Software Foundation, Software Carbon Intensity specification (ISO/IEC 21031:2024)**: the `carbon = E x I + M` frame, the location-based grid intensity requirement, and the obligation to disclose the methodology behind every figure.
- **Tsirogiannis, Harizopoulos, Shah, "Analyzing the Energy Efficiency of a Database Server", SIGMOD 2010**: database operators at the same CPU utilization can differ by up to 60% in power. Grounds the relative SQL verb weights only, which bill no database host, and the caveat on count-based ratios.
- **Xu, Tu, Wang, "Exploring Power-Performance Tradeoffs in Database Systems", ICDE 2010** and **Lella et al., "DBJoules: An Energy Measurement Tool for Database Management Systems", arXiv:2311.08961**: the relative energy cost of SQL operation classes behind the per-verb weighting, used as relative weights on the emitting service and never as database host energy.
- **Khan et al., "RAPL in Action: Experiences in Using RAPL for Power Measurements", ACM TOMPECS 2018**: RAPL readings correlate closely with external plug-power measurements, which is why RAPL-based backends rank above model-based ones.
- **Raffin, Trystram, "Dissecting the software-based measurement of CPU energy consumption: a comparative analysis", arXiv:2401.15985 (IEEE TPDS 2025)**: the pitfalls of software RAPL readers, and the reason `alumet_rapl` outranks `scaphandre_rapl`.
- **Raffin, Trystram, Richard, "Alumet: a Modular Framework to Standardize the Measurement of Energy Consumption", PECS 2025**: the measurement framework behind the recommended backend.
- **Pijnacker et al., "Container-level Energy Observability in Kubernetes Clusters", arXiv:2504.10702**: measured attribution errors in the legacy eBPF Kepler 0.7.2, before the 0.10 rewrite that Perf Sentinel reads. The **CNCF post "Kepler, re-architected" (June 2026)** reports the project's own validation of that rewrite, not an independent one, and states that Kepler attributes only active CPU usage per workload. `kepler_ebpf` sits below the RAPL backends on these current grounds, not on the 0.7.2 results.
- **Mytton, Lunden, Malmodin, "Network energy use not directly proportional to data volume", Journal of Industrial Ecology 28(4), 2024**: why the optional network transport term is modeled conservatively.
- **SPEC SPECpower_ssj2008 published results and the Cloud Carbon Footprint methodology**: the utilization-to-watts interpolation behind `cloud_specpower`.
- **Boavizta**: the server lifecycle assessments behind the embodied term.
- **Electricity Maps**: the live grid intensity API behind the `real_time` intensity source.
- **RTE éCO2mix** (open data on ODRÉ, Licence Ouverte v2.0): the free live intensity of the French grid, direct emissions of French production, behind `real_time` rows tagged `rte_eco2mix`.
- **NESO, Fingrid, Energinet** (CC BY 4.0): the free live intensity of the British, Finnish and Danish grids, behind `real_time` rows tagged `neso`, `fingrid` and `energinet`.
- **REE e·sios, Open Electricity, MAVIR, em6**: the free live intensity of the Spanish, Australian, Hungarian and New Zealand grids, under non-commercial or reserved terms quoted in `docs/CONFIGURATION.md`, behind `real_time` rows tagged `ree_esios`, `open_electricity`, `mavir` and `em6`.

The deeper design rationale, including how each backend's reading shape is normalized, lives in `docs/design/05-GREENOPS-AND-CARBON.md`.
