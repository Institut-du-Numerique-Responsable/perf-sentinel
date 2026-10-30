# Cartes à partager

Une carte à partager imprime le CO2 qu'une équipe a évité, mesuré par Perf Sentinel, à côté d'un équivalent du quotidien comme des repas ou des kilomètres en train. Cette page documente la table d'équivalences derrière ce second chiffre.

## Équivalences

Les équivalences viennent de la Base Carbone de l'ADEME. La table est embarquée dans le binaire, qui ne la télécharge jamais à l'exécution.

<!-- equivalences:begin -->
Millésime embarqué : Base Carbone `V23.6 (2025-07-03)`.

| `id` | Identifiant Base Carbone | Élément Base Carbone | Facteur | `per_unit_ug` | `unit` | `label` | Validité |
|---|---:|---|---|---:|---|---|---|
| `tgv_km` | 43256 | TGV, 2022 | 0.00293 kgCO2e/passager.km | 2_930_000 | `passenger_km` | km by high-speed train | avr-24 |
| `car_km` | 43791 | Voiture, Motorisation moyenne | 0.256 kgCO2e/km | 256_000_000 | `vehicle_km` | km driven by an average car | 2026 |
| `meal` | 20682 | Repas | 2.04 kgCO2e/repas | 2_040_000_000 | `meal` | average meals | déc-21 |
| `smartphone` | 27013 | Smartphone, de plus de 5,5 pouces | 39.1 kgCO2e/unité | 39_100_000_000 | `unit` | smartphones | 20/12/2021 |
<!-- equivalences:end -->

La Base Carbone garde une ligne `Valide générique` après la fin de sa période de validité, la dernière colonne montre donc l'âge de chaque facteur.

### Règles de sélection

1. Chaque ligne est une ligne `Elément` de la Base Carbone, `Valide générique`, localisée en `France continentale`, avec l'unité que le script attend pour elle.
2. L'unité est une chose du quotidien qui se compte : un kilomètre passager, un kilomètre, un repas, une unité. Jamais par euro, par kg ou par kWh.
3. Triée par facteur, chaque ligne vaut au plus 100 fois la précédente, pour qu'un chiffre de CO2 trouve toujours une ligne qui donne un nombre lisible.
4. Quand le libellé ne distingue pas les variantes, le facteur le plus haut l'emporte, pour que le nombre reste un « au moins ». La ligne smartphone est le modèle de plus de 5,5 pouces (39,1 kg), pas le « classique » (16,5 kg).
5. Pas d'avion, et aucune comparaison qui suggère un coût que la mesure ne montre pas.

`scripts/refresh-equivalences.py` applique les règles 1 et 2 et sort en 1 quand une ligne y échoue. Les tests appliquent la règle 3.

### Calcul du nombre

Le nombre part du CO2 tel que la carte l'imprime, jamais de la mesure brute. Le CO2 imprimé garde deux chiffres significatifs, arrondi vers le bas, et le nombre est la division entière de ce chiffre par le facteur. Une économie de 12,4 kg s'imprime « 12 kg » et 5 repas moyens (12 ÷ 2,04 = 5,88), là où les 12,4 kg bruts auraient donné 6. Les deux arrondis vont vers le bas, la carte n'annonce donc jamais plus que ce qui a été mesuré.

Quand l'appelant ne choisit pas d'équivalence, la carte prend le plus grand facteur qui compte encore au moins 2. Une équivalence choisie doit compter entre 2 et 1000.

### Attribution et licence

Chaque carte imprime le facteur utilisé, la source « ADEME, Base Carbone » et le millésime embarqué. La Base Carbone est publiée sous Licence Ouverte 2.0 (Etalab), qui autorise la réutilisation avec attribution.

### Rafraîchissement

```bash
python3 scripts/refresh-equivalences.py
cargo fmt -p perf-sentinel-core
```

Le script lit le jeu de données par l'API data.ademe.fr, réécrit `crates/sentinel-core/src/score/equivalence_data.rs` et la table entre les marqueurs `equivalences:begin` et `equivalences:end` de cette page et de sa version anglaise. Ne modifiez qu'en dehors des marqueurs. Le workflow `refresh-datasets` le lance deux fois par an et ouvre une pull request, et l'étape 2.5 de la release audite `EQUIVALENCE_TABLE_VINTAGE` (voir [`RELEASE-PROCEDURE-FR.md`](./RELEASE-PROCEDURE-FR.md)).
