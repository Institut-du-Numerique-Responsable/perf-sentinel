# Share cards

A share card prints the CO2 a team avoided, as measured by Perf Sentinel, next to an everyday equivalent such as meals or kilometres by train. This page documents the equivalence table behind that second figure.

## Equivalences

The equivalences come from the ADEME Base Carbone. The table ships embedded in the binary, which never fetches it at runtime.

<!-- equivalences:begin -->
Embedded vintage: Base Carbone `V23.6 (2025-07-03)`.

| `id` | Base Carbone id | Base Carbone element | Factor | `per_unit_ug` | `unit` | `label` | Validity |
|---|---:|---|---|---:|---|---|---|
| `tgv_km` | 43256 | TGV, 2022 | 0.00293 kgCO2e/passager.km | 2_930_000 | `passenger_km` | km by high-speed train | avr-24 |
| `car_km` | 43791 | Voiture, Motorisation moyenne | 0.256 kgCO2e/km | 256_000_000 | `vehicle_km` | km driven by an average car | 2026 |
| `meal` | 20682 | Repas | 2.04 kgCO2e/repas | 2_040_000_000 | `meal` | average meals | déc-21 |
| `smartphone` | 27013 | Smartphone, de plus de 5,5 pouces | 39.1 kgCO2e/unité | 39_100_000_000 | `unit` | smartphones | 20/12/2021 |
<!-- equivalences:end -->

Base Carbone keeps a row `Valide générique` after its validity period has lapsed, so the last column shows how old each factor is.

### Selection rules

1. Each row is a Base Carbone `Elément` line, `Valide générique`, located in `France continentale`, with the unit the script expects for it.
2. The unit is one everyday countable thing: a passenger kilometre, a kilometre, a meal, a unit. Never per euro, per kg or per kWh.
3. Sorted by factor, each row is at most 100 times the previous one, so a CO2 figure always has a row that gives a readable count.
4. When the label does not tell variants apart, the highest factor wins, so the count stays an "at least". The smartphone row is the over 5.5 inch model (39.1 kg), not the "classique" one (16.5 kg).
5. No flight, and no comparison implying a cost the measurement does not show.

`scripts/refresh-equivalences.py` enforces rules 1 and 2 and exits 1 when a row fails them. The tests enforce rule 3.

### How the count is computed

The count uses the CO2 as the card prints it, never the raw measurement. The printed CO2 keeps two significant figures, rounded down, and the count is a floor division of that figure by the factor. A saving of 12.4 kg prints "12 kg" and 5 average meals (12 ÷ 2.04 = 5.88), where the raw 12.4 kg would have given 6. Both roundings go down, so the card never claims more than was measured.

When the caller does not choose an equivalence, the card takes the largest factor that still counts at least 2. A chosen equivalence must count between 2 and 1000.

### Attribution and licence

Every card prints the factor it used, the source "ADEME, Base Carbone" and the embedded vintage. The Base Carbone is published under the Licence Ouverte 2.0 (Etalab), which allows reuse with attribution.

### Refresh

```bash
python3 scripts/refresh-equivalences.py
cargo fmt -p perf-sentinel-core
```

The script reads the dataset through the data.ademe.fr API, rewrites `crates/sentinel-core/src/score/equivalence_data.rs` and the table between the `equivalences:begin` and `equivalences:end` markers of this page and of its French mirror. Edit outside the markers only. The `refresh-datasets` workflow runs it twice a year and opens a pull request, and release step 2.5 audits `EQUIVALENCE_TABLE_VINTAGE` (see [`RELEASE-PROCEDURE.md`](./RELEASE-PROCEDURE.md)).
