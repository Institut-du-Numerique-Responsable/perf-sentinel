# Référence de configuration

Perf Sentinel se configure via un fichier `.perf-sentinel.toml`. Tous les champs sont optionnels et ont des valeurs par défaut raisonnables.

<img alt="Vue d'ensemble des commandes CLI" src="https://raw.githubusercontent.com/Institut-du-Numerique-Responsable/perf-sentinel/main/docs/diagrams/svg/cli-commands.svg">

## Sommaire

- [Fragments de configuration](#fragments-de-configuration) : chargement multi-fichier déterministe.
- [Sous-commandes](#sous-commandes) : quelles sous-commandes lisent `.perf-sentinel.toml`.
- [Sections](#sections) : référence complète par section (`[thresholds]`, `[detection]`, `[green]`, `[daemon]`, `[reporting]`).
- [Configuration minimale](#configuration-minimale) : le `.perf-sentinel.toml` le plus court utile.
- [Exemple de configuration complète](#exemple-de-configuration-complète) : chaque section remplie avec des valeurs d'exemple.
- [Migration depuis 0.5.x](#migration-depuis-05x) : les 8 clés top-level legacy retirées en 0.6.0 et comment migrer.
- [Variables d'environnement](#variables-denvironnement) : quelles variables d'environnement surchargent les valeurs du fichier de config.

## Fragments de configuration

Perf Sentinel charge les documents TOML du répertoire `.perf-sentinel.d/`
placé à côté de la configuration principale, puis charge
`.perf-sentinel.toml` en dernier. Cette règle vaut aussi avec
`--config chemin/vers/custom.toml` : les fragments viennent de
`chemin/vers/.perf-sentinel.d/` et `custom.toml` reste la surcharge finale.
Le fichier principal est facultatif uniquement sans option `--config`.
Les valeurs par défaut sont utilisées uniquement si ni le fichier principal
implicite ni aucun fragment n'existe. Un fichier illisible ou une erreur de
syntaxe TOML individuelle arrête la commande avec le code 75. Après
application de toutes les surcharges, la configuration fusionnée doit
passer la désérialisation typée et la validation, sinon la commande
s'arrête aussi.

Le nom d'un fragment doit suivre `NN-nom-minuscule.toml`, avec une priorité
unique `NN` comprise entre `00` et `99`. Les fichiers sont chargés par priorité
croissante. Une priorité dupliquée, une majuscule ou un séparateur ambigu est
refusé. Les fichiers qui ne se terminent pas par `.toml` sont ignorés.

Lorsque les deux valeurs sont des tables, elles fusionnent récursivement.
Toute autre valeur plus tardive remplace la précédente à la même clé. Le
document fusionné final doit toujours respecter le schéma de configuration
typé. Depuis la 0.12.0 ce schéma est strict : une clé ou un nom de table
qu'aucune section ne déclare fait échouer le chargement au lieu d'être
ignoré, donc un réglage mal orthographié arrête la commande au lieu de
laisser silencieusement la valeur par défaut en place. Les exemples réservent
ces plages :

| Priorité    | Usage                                                    |
|-------------|----------------------------------------------------------|
| `00` à `19` | valeurs partagées, seuils et détection                   |
| `20` à `39` | sources d'énergie et mesure GreenOps                     |
| `40` à `49` | sources d'intensité carbone                              |
| `50` à `69` | daemon et topologie de déploiement                       |
| `70` à `89` | reporting et politique propre à l'organisation           |
| `90` à `99` | surcharges locales, à garder de préférence hors du dépôt |

Les fragments prêts à copier dans `examples/` conservent leur priorité dans
leur nom. Gardez ces noms en les copiant dans `.perf-sentinel.d/` :
`30-green-alumet.toml`, `31-green-cloud.toml`,
`32-green-scaphandre.toml`, `33-green-kepler.toml`,
`34-green-redfish.toml` et `40-green-electricity-maps.toml`.
`60-daemon-docker.toml` est une configuration principale autonome pour les
topologies Compose collector et sharded. Montez-le comme `.perf-sentinel.toml`,
puis placez uniquement les fragments GreenOps optionnels dans le répertoire
frère `.perf-sentinel.d/`.

`perf-sentinel watch` ajoute une couche au-dessus de tous les fichiers. Ses
options `--listen-address`, `--listen-port-http`, `--listen-port-grpc` et
`--max-export-findings` entrent dans la fusion comme un dernier document
`[daemon]`, donc une option remplace la même clé venue d'un fragment ou du
fichier principal, même si le fichier donne à cette clé un type incorrect. La
validation s'exécute une seule fois, sur ce résultat : une option passe par
les mêmes bornes que le fichier, une option hors bornes arrête le daemon avec
le code 75 et une erreur qui nomme les options de la ligne de commande, et
chaque avertissement s'affiche une seule fois, avec les valeurs qu'utilise le
daemon. Une valeur que clap refuse lui-même, comme un port au-delà de 65535,
sort avec le code 2 avant la lecture de tout fichier.

## Sous-commandes

| Sous-commande  | Description                                                                                                                                                                                                                                 |
|----------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `analyze`      | Analyse batch de fichiers de traces. Lit depuis un fichier ou stdin                                                                                                                                                                         |
| `explain`      | Vue arborescente d'une trace avec findings annotés en ligne                                                                                                                                                                                 |
| `watch`        | Mode daemon : ingestion OTLP temps réel et détection en streaming                                                                                                                                                                           |
| `query`        | Interroge un daemon en cours d'exécution sur ses findings, ses corrélations ou son statut. Sortie colorée par défaut, `--format json` pour le scripting. `query inspect` ouvre un TUI live                                                  |
| `demo`         | Lance l'analyse sur un jeu de données de démo embarqué                                                                                                                                                                                      |
| `bench`        | Benchmark du débit sur un fichier de traces                                                                                                                                                                                                 |
| `pg-stat`      | Analyse des exports `pg_stat_statements` (CSV/JSON ou Prometheus)                                                                                                                                                                           |
| `inspect`      | TUI interactif pour parcourir les traces, les findings et les arbres de spans                                                                                                                                                               |
| `diff`         | Compare deux jeux de traces et émet un rapport delta (findings nouveaux/résolus, changements de sévérité, deltas I/O par endpoint). Sortie texte/JSON/SARIF                                                                                 |
| `report`       | Dashboard HTML single-file pour l'exploration post-mortem dans n'importe quel navigateur. Accepte un fichier de traces, un Report JSON pré-calculé, ou stdin via `--input -` (auto-détecte array-d'events vs objet Report, tolérant au BOM) |
| `tempo`        | Récupère des traces depuis une API HTTP Grafana Tempo (par ID de trace ou par recherche service puis récupération) et les fait passer dans le pipeline d'analyse. Conditionné à la feature `tempo`                                          |
| `jaeger-query` | Récupère des traces depuis n'importe quel backend qui parle l'API de requête Jaeger (Jaeger, Victoria Traces) et les fait passer dans le pipeline d'analyse. Conditionné à la feature `jaeger-query`                                        |
| `calibrate`    | Corrèle un fichier de traces avec des mesures d'énergie prises sur l'hôte propre de chaque service (Scaphandre, CSV cloud monitoring) et émet un TOML de coefficients I/O-vers-énergie à charger via `[green] calibration_file`. Une série portée par plusieurs services (mêmes horodatages et mêmes valeurs) est signalée, car chaque facteur chiffrerait la lecture entière : la garder sur un seul service ou la répartir avant |

## Sections

### `[thresholds]`

Seuils du quality gate. Le quality gate échoue si une règle est violée.

| Champ                              | Type     | Défaut | Description                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
|------------------------------------|----------|--------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `n_plus_one_sql_critical_max`      | entier   | `0`    | Nombre maximum de findings N+1 SQL **critiques** avant l'échec du gate                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `n_plus_one_http_warning_max`      | entier   | `3`    | Nombre maximum de findings N+1 HTTP **warning ou plus** avant l'échec du gate                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `n_plus_one_messaging_warning_max` | entier   | `3`    | Nombre maximum de findings N+1 messaging **warning ou plus** avant l'échec du gate. Warning+ plutôt que critique seul, comme HTTP : un client Kafka peut déjà grouper les publications qu'il met en tampon, le compte d'occurrences y est donc un majorant                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `io_waste_ratio_max`               | flottant | `0.30` | Ratio maximum de gaspillage I/O (0.0 à 1.0) avant l'échec du gate                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `min_usable_span_ratio`            | flottant | absent | Part minimale (0.0 à 1.0) de spans de forme I/O qui doivent être analysables (spans SQL portant `db.statement`, spans HTTP CLIENT portant une URL complète) avant l'échec du gate, calculée par nature d'I/O et rapportée comme la pire d'entre elles, pour qu'un trafic HTTP sain ne masque pas une surface SQL cassée. Une nature d'I/O portant moins de 20 spans n'est pas jugée, échantillon trop petit pour un ratio qui bloque un build. Quand aucune nature ne franchit ce plancher la règle est ignorée et le rapport porte un avertissement `tuning` qui le dit. Runs batch uniquement (`analyze`, `report`), le daemon n'exporte pas de décompte. Garde-fou contre un faux vert dû à une instrumentation inexploitable : sous le seuil, le gate échoue même avec zéro finding. Absent = règle désactivée. S'applique à l'entrée OTLP seulement, qui porte le décompte de filtrage par raison (exposé dans `analysis.ingest` du rapport JSON) |

### `[detection]`

Paramètres des algorithmes de détection.

| Champ                                  | Type   | Défaut                                        | Description                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
|----------------------------------------|--------|-----------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `n_plus_one_min_occurrences`           | entier | `5`                                           | Nombre minimum d'occurrences (avec des paramètres distincts) pour signaler un pattern N+1                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| `window_duration_ms`                   | entier | `500`                                         | Fenêtre temporelle en millisecondes dans laquelle les opérations répétées sont considérées comme un pattern N+1                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `slow_query_threshold_ms`              | entier | `500`                                         | Seuil de durée en millisecondes au-dessus duquel une opération est considérée comme lente                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| `slow_query_min_occurrences`           | entier | `3`                                           | Nombre minimum d'occurrences lentes du même template pour générer un finding. Dans le daemon, s'applique aussi aux épisodes lents dans la fenêtre `slow_query_window_minutes`                                                                                                                                                                                                                                                                                                                                                       |
| `slow_query_window_minutes`            | entier | `15`                                          | Daemon uniquement. Fenêtre en minutes sur laquelle les épisodes lents d'un même (type, service, grouping, template) sont comptés d'un batch d'analyse à l'autre. Les spans lents situés à moins de max(60 s, 1,5 x `trace_ttl_ms`) du premier span d'un épisode comptent pour un seul épisode, donc un span lent isolé ou un verrou bref ne produit pas de finding. `0` désactive (plage : 0-60)                                                                                                                                    |
| `max_fanout`                           | entier | `20`                                          | Nombre maximum de spans enfants par parent avant de signaler un fanout excessif (plage : 1-100000)                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| `chatty_service_min_calls`             | entier | `15`                                          | Une trace est signalée comme service bavard quand ses appels HTTP sortants dépassent cette valeur. Sévérité : warning > seuil, critical > 3x seuil.                                                                                                                                                                                                                                                                                                                                                                                 |
| `pool_saturation_concurrent_threshold` | entier | `10`                                          | Pic de spans SQL concurrents par service au sein d'une même trace à partir duquel signaler un risque de saturation du pool de connexions. Utilise un algorithme de balayage sur les horodatages des spans.                                                                                                                                                                                                                                                                                                                          |
| `serialized_min_sequential`            | entier | `3`                                           | Nombre minimum d'appels séquentiels indépendants (même parent, sans chevauchement, templates différents) pour signaler des appels potentiellement parallélisables.                                                                                                                                                                                                                                                                                                                                                                  |
| `grouping_attributes`                  | liste  | `["k8s.namespace.name", "service.namespace"]` | Attributs de ressource ou de span qui séparent un déploiement d'un autre, du plus spécifique au moins. Le premier présent sur un span décide de l'identité du finding, le même problème dans deux namespaces reste donc deux findings. Chaque attribut présent est capturé et affiché. 8 entrées max, une liste vide désactive le regroupement. Les signatures d'acquittement l'ignorent. Depuis 0.19.0 la valeur effective est aussi le label `grouping` des métriques par service du daemon, voir `[daemon] per_grouping_labels`. |
| `sanitizer_aware_classification`       | chaîne | `"auto"`                                      | Classification des groupes SQL dont les littéraux ont été remplacés par un placeholder (`?`, `$?`, `%s`, `@param`, `:name`) par un agent OTel ou un driver de base de données. Une valeur parmi `"auto"`, `"strict"`, `"always"`, `"never"`. Voir la note ci-dessous.                                                                                                                                                                                                                                                               |
| `sanitizer_aware_min_cv`               | nombre | `0.5`                                         | Coefficient de variation (écart-type sur moyenne) des durées par span au-dessus duquel l'heuristique sanitizer lit un groupe comme un N+1 plutôt qu'une répétition en cache. Plage `(0, 10]`. Voir la note ci-dessous.                                                                                                                                                                                                                                                                                                              |

#### `sanitizer_aware_classification`

Les agents OpenTelemetry et les drivers de base de données activent par
défaut la sanitization des instructions SQL pour éviter de laisser
fuir des PII dans les attributs de trace. Le style de placeholder
dépend de la stack : les agents JDBC produisent un `?` nu, les drivers
PostgreSQL natifs (pgx, asyncpg, sqlx) produisent `$1`/`$2`
(normalisés en `$?`), les drivers Python DB-API produisent `%s`, les
drivers .NET produisent `@p0`/`@Name`, et Oracle/SQLAlchemy
produisent `:name`. Dans tous les cas, les spans arrivent dans
perf-sentinel avec le même template et aucun paramètre extractible. La
règle standard de paramètres distincts rejette donc le groupe et le
détecteur de redondance le récupère sous l'étiquette `redundant_sql` au
lieu de `n_plus_one_sql`. Ce paramètre contrôle l'heuristique qui
restaure la classification correcte :

- `"auto"` (défaut) : émet `n_plus_one_sql` quand **soit** le signal de
  scope ORM se déclenche (Spring Data, Hibernate, EF Core, SQLAlchemy,
  ActiveRecord, GORM, Prisma, Diesel, Laravel/Eloquent, Doctrine, ...),
  **soit** la variance des durées par span est suffisante pour indiquer
  des accès à des lignes distinctes. Sinon, le groupe reste à la charge
  du détecteur de redondance. Meilleur rappel sur les stacks ORM de
  production Spring Data, EF Core et similaires.
- `"strict"` : reclassifie uniquement quand un signal primaire
  (marqueur de scope ORM, nombre d'occurrences >= 3 x
  `n_plus_one_min_occurrences`, ou siblings séquentiels) se déclenche
  conjointement avec un signal corroboratif (variance temporelle
  élevée ou nombre d'occurrences élevé). Préserve la précision de
  `redundant_sql` sur les requêtes identiques mises en cache, en nombre
  modéré (boucles de polling legacy, recherches de config non
  mémoïsées, typiquement 5-10 appels par requête). Au-dessus de la
  barre de haute occurrence (par défaut 15), tout groupe sanitisé se
  déclenche quel que soit le scope ORM, les siblings séquentiels ou la
  variance, sous la garde `looks_sanitized`. À utiliser quand les
  findings `redundant_sql` sont un signal exploitable qui ne doit pas
  être absorbé silencieusement par `n_plus_one_sql`. Le laboratoire de
  simulation fait tourner toutes ses stacks ainsi, parce que sous `auto`
  un marqueur de scope ORM suffit à reclassifier en N+1 une répétition
  de la même requête servie par un cache. Ce changement de verdict ne
  vaut que pour des nombres d'occurrences modérés, la barre de haute
  occurrence ci-dessus se déclenchant aussi sous `strict`.
- `"always"` : reclassifie tout groupe sanitisé qui atteint
  `n_plus_one_min_occurrences` spans en `n_plus_one_sql`. Plus agressif,
  peut requalifier une vraie redondance à un seul paramètre.
- `"never"` : désactive complètement l'heuristique et se rabat sur la
  vérification stricte `distinct_params`.

Les findings reclassifiés par l'heuristique (sous `"auto"`, `"strict"`
ou `"always"`) portent `classification_method = "sanitizer_heuristic"`
dans leur représentation JSON, ce qui permet à un opérateur de repérer
où elle se déclenche. Les findings produits par la règle standard
omettent ce champ.

#### `sanitizer_aware_min_cv`

Le signal de variance temporelle derrière les modes ci-dessus compare le
coefficient de variation des durées par span du groupe à ce seuil. Des
recherches sur des clés différentes étalent leurs durées entre hits et
miss de cache, alors que les répétitions d'une même requête en cache se
regroupent. Le défaut de `0.5` préfère signaler un N+1 plutôt qu'en
manquer un, puisqu'une erreur ne fait qu'échanger `redundant_sql` contre
`n_plus_one_sql` au même poids d'I/O évitables.

À relever sur un runtime dont la gigue d'ordonnancement étale même les
répétitions en cache : workers PHP-FPM, conteneurs bridés en CPU,
runners CI partagés. Le laboratoire de simulation a mesuré un CV
d'environ 0,75 sur dix recherches Doctrine identiques servies depuis le
cache sous charge. C'est assez pour franchir le défaut et transformer un
finding `redundant_sql` en `n_plus_one_sql`, avec une piste de
remédiation (`leftJoin`, `with()`) qui ne s'applique pas à une
répétition. À `1.0` le groupe garde son verdict `redundant_sql`.

Le même seuil alimente l'heuristique HTTP, qui décide si un appel sortant
répété avec peu de paramètres distincts se lit `n_plus_one_http` ou
`redundant_http`, si bien que le relever déplace les deux verdicts. Ce
qui continue de signaler un vrai N+1 quelle que soit la variance dépend
du mode et du chemin :

- SQL sous `"auto"` : le marqueur de scope ORM à lui seul, donc relever
  le seuil ne change rien sur un groupe instrumenté par un ORM.
- SQL sous `"strict"` : la barre de haute occurrence
  (`3 x n_plus_one_min_occurrences`).
- HTTP : la règle directe, puisque des paramètres de chemin ou de
  requête distincts classent le groupe avant que l'heuristique ne tourne.
- `"always"` ignore la variance entièrement, `"never"` ne la consulte
  jamais.

Une seule valeur sert toute la configuration : un daemon devant plusieurs
runtimes prend le seuil du plus bruyant et accepte la perte sur les
groupes de compte modéré ailleurs.

La valeur est consignée dans le `detection_config` de chaque rapport. Un
rapport écrit avant l'existence de la clé se relit avec `0.5`, la valeur
que son run avait en dur.

### `[green]`

> **Voir aussi.** L'[introduction énergie et SCI](METHODOLOGY-FR.md#introduction--énergie-et-sci-v10) dans la doc méthodologie définit SCI v1.0 (termes E + I + M), RAPL, Scaphandre, SPECpower, Boavizta et l'API Electricity Maps utilisés par les sections de config ci-dessous. À lire une fois si l'un des termes ne vous est pas familier.

Configuration du scoring GreenOps alignée sur [SCI v1.0](https://github.com/Green-Software-Foundation/sci) (termes opérationnel + embarqué, intervalles de confiance, multi-région).

| Champ                              | Type     | Défaut    | Description                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
|------------------------------------|----------|-----------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `enabled`                          | booléen  | `true`    | Active le scoring GreenOps (IIS, ratio de gaspillage, top offenders, CO₂)                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `default_region`                   | chaîne   | *(aucun)* | Région cloud de repli utilisée quand ni l'attribut `cloud.region` du span ni le mapping `service_regions` ne résout une région. Exemples : `"eu-west-3"`, `"us-east-1"`, `"FR"`                                                                                                                                                                                                                                                                                                                                                   |
| `embodied_carbon_per_request_gco2` | flottant | `0.001`   | Terme `M` SCI v1.0 : émissions de fabrication matérielle amorties par requête (par trace), en gCO₂eq. Indépendant de la région. Un zéro est déprécié depuis la 0.9.25 et se rabat sur ce défaut avec un avertissement : aucun matériel n'a un carbone incorporé nul, et un coefficient à zéro effaçait le terme M de la divulgation. La valeur appliquée est publiée dans `calibration_inputs`                                                                                                                                    |
| `use_hourly_profiles`              | booléen  | `true`    | Quand `true`, l'étape de scoring utilise des intensités réseau spécifiques à l'heure pour les 30+ régions disposant de profils horaires embarqués. Les régions avec profils mois x heure (FR, DE, GB, US-East) prennent aussi en compte la variation saisonnière. Les rapports sont étiquetés `model = "io_proxy_v3"` (mois x heure) ou `"io_proxy_v2"` (horaire annuel plat). Mettre à `false` pour figer les rapports sur le modèle annuel plat                                                                                 |
| `hourly_profiles_file`             | chaîne   | *(aucun)* | Chemin vers un fichier JSON de profils horaires personnalisés. Peut être absolu ou relatif au fichier de config. Les profils personnalisés prennent priorité sur les profils embarqués pour la même clé de région. Voir "Profils horaires fournis par l'utilisateur" ci-dessous                                                                                                                                                                                                                                                   |
| `per_operation_coefficients`       | booléen  | `true`    | Quand `true`, le modèle proxy pondère l'énergie par opération I/O selon le type d'opération : SQL SELECT (0.5x), INSERT/UPDATE (1.5x), DELETE (1.2x) et tailles de payload HTTP (petit <10 Ko : 0.8x, moyen 10 Ko-1 Mo : 1.2x, grand >1 Mo : 2.0x). Ne s'applique pas quand un backend mesuré fournit l'énergie du service. Mettre à `false` pour utiliser le coefficient plat `ENERGY_PER_IO_OP_KWH` pour toutes les opérations                                                                                                  |
| `include_network_transport`        | booléen  | ignoré    | Dépréciée et ignorée depuis la 0.9.25 : le terme de transport est toujours calculé, toujours affiché et toujours publié, un interrupteur d'affichage sur un chiffre publié n'avait plus de justification. La clé se parse encore, avec un avertissement. Le terme requiert `response_size_bytes` sur les spans HTTP (attribut OTel `http.response.body.size`) et la région cible mappée via `[green.service_regions]`. Les appels intra-région sont exclus. Le CO₂ transport apparaît comme `transport_gco2` dans le rapport JSON |
| `network_energy_per_byte_kwh`      | flottant | ignoré    | Dépréciée et ignorée depuis la 0.9.25. Le coefficient est fixé à 0.04 kWh/Go pour que chaque divulgation mette le transport à la même échelle, et la divulgation publie la fourchette sourcée 0.001-0.059 kWh/Go à côté (voir LIMITATIONS-FR.md, transport réseau). La clé se parse encore, avec un avertissement                                                                                                                                                                                                                 |

#### `[green.service_regions]`

Surcharges de région par service utilisées quand `cloud.region` OTel est absent des spans (ex. ingestion Jaeger / Zipkin). Associe un nom de service à une clé de région. Les noms de service se comparent sans tenir compte de la casse, donc deux clés qui ne diffèrent que par la casse et pointent vers des régions différentes sont refusées au chargement.

```toml
[green]
default_region = "eu-west-3"
embodied_carbon_per_request_gco2 = 0.001

[green.service_regions]
"order-svc" = "us-east-1"
"chat-svc"  = "ap-southeast-1"
```

#### Chaîne de résolution de région

Pour chaque span, l'étape de scoring carbone résout la région effective dans cet ordre (la première correspondance l'emporte) :

1. **`event.cloud_region`** : depuis l'attribut de ressource OTel `cloud.region` (ou attribut de span en repli). La source qui fait le plus autorité.
2. **`[green.service_regions][event.service]`** : surcharge config par service.
3. **`[green] default_region`** : repli global.

Les ops I/O sans région résolvable atterrissent dans un bucket synthétique `"unknown"` avec zéro CO₂ opérationnel. La ligne apparaît dans `regions[]` pour la visibilité. Le carbone embarqué est tout de même émis car les émissions de fabrication matérielle sont indépendantes de la région. La cardinalité des régions est plafonnée à 256 buckets distincts. Le surplus tombe dans le bucket `unknown` pour éviter l'épuisement mémoire en cas d'ingestion mal configurée.

#### Forme de sortie

Quand le scoring vert est activé et qu'au moins un événement est analysé, le `green_summary` du rapport JSON inclut :

- **`co2`** : objet structuré `{ total, avoidable, operational_gco2, embodied_gco2 }`, plus `transport_gco2`, `database_gco2` et `messaging_gco2` quand ils sont présents (les deux derniers sont le carbone des hôtes de base de données et de broker qu'une fenêtre du daemon a compté dans `total`). `total` et `avoidable` sont tous deux `{ low, mid, high, model, methodology }` avec une **incertitude multiplicative 2×** (`low = mid/2`, `high = mid×2`). Le tag `methodology` distingue `total` (`"sci_v1_numerator+transport"` : `(E × I) + M + T` sommé sur les traces, y compris quand `T` vaut zéro) de `avoidable` (`"sci_v1_operational_ratio"` : ratio global aveugle à la région, exclut le carbone embarqué et le transport). Une fenêtre du daemon scorée avec une source d'hôte de base de données ou de broker configurée porte à la place `"sci_v1_numerator+transport+hosts"` et `"sci_v1_operational_ratio+hosts"`, énergie d'hôte nulle comprise. Les rapports historiques peuvent porter `"sci_v1_numerator"`. Valeurs `model`, le plus précis gagne : `"electricity_maps_api"` > `"grid_operator_api"` > `"alumet_rapl"` > `"scaphandre_rapl"` > `"kepler_ebpf"` > `"redfish_bmc"` > `"cloud_specpower"` > `"io_proxy_v3"` > `"io_proxy_v2"` > `"io_proxy_v1"`. Quand des facteurs de calibration sont actifs sur les modèles proxy, `+cal` est ajouté (ex. `"io_proxy_v2+cal"`). Le suffixe `+cal` ne s'applique jamais à un tag mesuré.
- **`regions[]`** : détail par région avec `{ region, grid_intensity_gco2_kwh, pue, io_ops, co2_gco2, intensity_source }`, **trié par `co2_gco2` décroissant** (régions à plus fort impact en premier) avec départage alphabétique. `intensity_source` vaut `"annual"`, `"hourly"`, `"monthly_hourly"` ou `"real_time"` (API Electricity Maps) selon la source d'intensité carbone utilisée pour la région.

Les données d'intensité carbone sont embarquées dans le binaire (aucun appel réseau sortant). Voir `docs/FR/design/05-GREENOPS-AND-CARBON-FR.md` pour la formule complète et la méthodologie et [docs/FR/LIMITATIONS-FR.md](LIMITATIONS-FR.md#précision-des-estimations-carbone) pour l'avertissement directionnel / non-réglementaire.

#### Profils horaires fournis par l'utilisateur

Pointez `[green] hourly_profiles_file` vers un fichier JSON pour fournir vos propres profils horaires. C'est utile pour les opérateurs de datacenter qui ont leurs propres contrats d'achat d'électricité (PPA) ou pour surcharger les données embarquées avec des mesures locales.

```json
{
  "profiles": {
    "my-datacenter": {
      "type": "flat_year",
      "hours": [45.0, 44.0, 43.0, "... 24 values total ..."]
    },
    "eu-west-3": {
      "type": "monthly",
      "months": [
        [50.0, 49.0, "... 24 values for January ..."],
        ["... 11 more months ..."]
      ]
    }
  }
}
```

Les profils fournis par l'utilisateur ont priorité sur les profils embarqués pour la même clé de région. Validation au chargement de la config : chaque `flat_year` doit contenir exactement 24 valeurs, chaque `monthly` doit contenir exactement 12 tableaux de 24 valeurs. Toutes les valeurs doivent être finies et non-négatives. Si la clé de région existe dans la table carbone embarquée, un avertissement est journalisé quand la moyenne du profil s'écarte de plus de 5% de la valeur annuelle, mais le profil est quand même accepté.

#### Alias de régions pour les profils horaires

Les alias de code pays et les synonymes de fournisseurs cloud résolvent vers le même profil horaire. Par exemple, `"fr"`, `"francecentral"` et `"europe-west9"` renvoient tous au profil `eu-west-3` (France). Correspondances notables :

- `"us"`, `"eastus"` → `us-east-1` (US-East, la région de déploiement US la plus courante)
- `"westeurope"`, `"nl"`, `"nl-ams"` → `eu-west-4` (Pays-Bas)
- `"northeurope"`, `"ie"` → `eu-west-1` (Irlande)
- `"uksouth"`, `"gb"`, `"uk"`, `"uk1"` → `eu-west-2` (Royaume-Uni)
- `"westus2"` → `us-west-2` (Oregon)
- `"gra11"`, `"gra"`, `"sbg"`, `"fr-par"`, `"outscale-eu-west-2"` → `eu-west-3` (France)
- `"waw1"`, `"pl-waw"` → `europe-central2` (Pologne)
- `"bhs5"`, `"bhs"` → `ca-central-1` (Québec)

**Clés OVHcloud, Scaleway et OUTSCALE.** OVHcloud nomme un même datacenter de trois façons selon l'API interrogée (`GRA11` pour la région OpenStack Public Cloud, `GRA` pour le code de zone, `gra` pour la chaîne de localisation S3), et les trois sont référencées. Les clés OUTSCALE portent un préfixe `outscale-` parce qu'OUTSCALE réutilise les identifiants de région d'AWS pour d'autres lieux : son `eu-west-2` est Paris là où celui d'AWS est Londres. Un déploiement OUTSCALE déclare donc `default_region = "outscale-eu-west-2"` et non l'identifiant nu, sinon il est évalué sur le réseau britannique.

La table complète des alias se trouve dans `score/carbon_profiles.rs`. Si votre clé de région n'a pas d'alias, la valeur annuelle plate de la table carbone principale est utilisée.

**Tous les backends d'énergie et d'intensité réseau sont réservés au daemon.** `[green.alumet]`, `[green.scaphandre]`, `[green.kepler]`, `[green.redfish]`, `[green.cloud]`, `[green.database_static]`, `[green.broker_static]` et `[green.electricity_maps]` sont interrogés par le daemon `watch` et par rien d'autre. Un run batch `analyze` ou `report` ne démarre aucun scraper : il calcule donc avec l'estimation proxy I/O sur les données d'intensité embarquées quoi que disent ces sections, et il émet un avertissement nommant les sections qu'il a dû ignorer. Le batch ne scrape pas, car attribuer une puissance mesurée maintenant à des traces enregistrées plus tôt produirait un chiffre faux plutôt qu'un chiffre absent.

#### `[green.scaphandre]` (optionnel, opt-in)

Intégration opt-in avec [Scaphandre](https://github.com/hubblo-org/scaphandre) pour la mesure énergétique par processus sur les hôtes Linux avec prise en charge d'Intel RAPL. Quand cette section est configurée, le daemon `watch` lance une tâche de fond qui scrape l'endpoint Prometheus de Scaphandre toutes les `scrape_interval_secs` secondes et utilise les lectures de puissance mesurées pour remplacer la constante `ENERGY_PER_IO_OP_KWH` fixe pour chaque service mappé.

**Préférez `[green.alumet]` pour les nouveaux déploiements.** Les deux intégrations lisent les mêmes compteurs RAPL, mais l'échantillonnage d'Alumet est mesurablement moins erroné, comme le caractérisent ses propres auteurs dans [Dissecting the software-based measurement of CPU energy consumption](https://hal.science/hal-04420527v2/document) (Raffin et al.), et il attribue par cgroup plutôt que par processus. La prise en charge de Scaphandre est conservée pour les déploiements existants, et `alumet_rapl` surclasse `scaphandre_rapl` dès que les deux alimentent le même service. Voir [docs/FR/LIMITATIONS-FR.md](LIMITATIONS-FR.md#limites-de-précision-scaphandre).

| Champ                  | Type   | Défaut    | Description                                                                                                                                                                                 |
|------------------------|--------|-----------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `endpoint`             | chaîne | *(aucun)* | URL complète de l'endpoint Prometheus `/metrics` de Scaphandre. Doit commencer par `http://` ou `https://` (TLS pris en charge via hyper-rustls). Obligatoire quand la section est présente |
| `scrape_interval_secs` | entier | `5`       | Fréquence de scrape, en secondes. Plage valide : 1-3600                                                                                                                                     |
| `process_map`          | table  | `{}`      | Associe les noms de service Perf Sentinel (depuis `service.name` du span) à un `ProcessMatcher` par service (voir ci-dessous)                                                               |

Chaque entrée `process_map` est une table avec deux champs : `exe_contains` (obligatoire, sous-chaîne comparée au label `exe` de Scaphandre) et `cmdline_contains` (optionnel, sous-chaîne comparée au label `cmdline`). Quand `cmdline_contains` est défini, la correspondance exige que les deux sous-chaînes soient présentes. Exactement un processus Scaphandre doit correspondre à chaque entrée, sinon l'étape de scoring saute ce service pour le tick et émet un log `warn` nommant l'ambiguïté. Des entrées dont les matchers sélectionnent le même processus se partagent sa puissance selon les opérations que chaque service a émises dans la fenêtre de scrape.

```toml
[green.scaphandre]
endpoint = "http://localhost:8080/metrics"
scrape_interval_secs = 5

[green.scaphandre.process_map."order-svc"]
exe_contains = "bin/java"
cmdline_contains = "order-svc.jar"

[green.scaphandre.process_map."chat-svc"]
exe_contains = "bin/java"
cmdline_contains = "chat-svc.jar"

[green.scaphandre.process_map."native-svc"]
exe_contains = "/opt/native-svc/bin/native-svc"
```

**Pourquoi `exe_contains` ET `cmdline_contains`.** Scaphandre émet `exe` comme chemin absolu du runtime (`/usr/lib/jvm/.../bin/java`, `/usr/share/dotnet/dotnet`). Plusieurs services co-localisés partageant un runtime (plusieurs JVMs, plusieurs assemblies .NET) entrent en collision sur `exe`, et seul `cmdline` les distingue. Scaphandre concatène en plus argv sans séparateurs : `java -jar /tmp/order-svc.jar` apparaît comme `cmdline="java-jar/tmp/order-svc.jar"`. Configurez `cmdline_contains` avec une sous-chaîne présente dans cette forme concaténée (par exemple le nom du jar ou de la dll), PAS avec une ligne de commande POSIX contenant des espaces.

**Ignoré en mode batch `analyze`.** Seul le daemon `watch` lance le scraper. La commande `analyze` utilise toujours le modèle proxy quelle que soit cette section.

**Comportement de repli.** Quand l'endpoint est inaccessible, qu'un service n'est pas présent dans `process_map` ou qu'un service a eu zéro ops dans la fenêtre de scrape courante, l'étape de scoring se rabat sur le modèle proxy pour ces spans. Le premier échec est journalisé en niveau `warn`, les échecs suivants en `debug` pour éviter le spam. La jauge Prometheus `perf_sentinel_scaphandre_last_scrape_age_seconds` permet aux opérateurs de détecter un scraper bloqué.

**Limites de précision (important).** Scaphandre améliore le coefficient énergétique **au niveau service** mais ne donne PAS d'attribution par finding. RAPL est au niveau processus, pas au niveau span : deux findings dans le même processus pendant la même fenêtre de scrape partagent le même coefficient. Voir [docs/FR/LIMITATIONS-FR.md](LIMITATIONS-FR.md#limites-de-précision-scaphandre) pour la discussion complète.

#### `[green.kepler]` (optionnel, opt-in)

Intégration opt-in avec [Kepler](https://github.com/sustainable-computing-io/kepler) (projet CNCF sandbox) pour la mesure d'énergie par conteneur ou par processus. Perf Sentinel lit Kepler 0.10 et suivants, qui lit RAPL via sysfs, avec hwmon en repli expérimental depuis la v0.12.0, et répartit la puissance active entre les charges au prorata du temps CPU. Une fois configuré, le daemon `watch` scrape l'endpoint Prometheus `/metrics` de Kepler, calcule le delta de joules par service par rapport au scrape précédent, et publie un coefficient mesuré par opération étiqueté `kepler_ebpf`. Le nom du tag est historique : depuis Kepler 0.10, seule version que lit Perf Sentinel, la source est RAPL (sysfs/powercap, hwmon expérimental), pas eBPF.

| Champ                  | Type   | Défaut        | Description                                                                                                                                                                     |
|------------------------|--------|---------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `endpoint`             | chaîne | *(aucun)*     | URL complète de l'endpoint Prometheus `/metrics` de Kepler. Obligatoire quand la section est présente.                                                                          |
| `scrape_interval_secs` | entier | `5`           | Fréquence de scrape en secondes. Plage valide : 1-3600.                                                                                                                         |
| `metric_kind`          | chaîne | `"container"` | Compteur Kepler v2 à lire : `"container"` (`kepler_container_cpu_joules_total`, clé `container_name`) ou `"process"` (`kepler_process_cpu_joules_total`, clé `comm`).           |
| `zone`                 | chaîne | `"package"`   | Valeur du label `zone` à lire. Kepler expose une série par zone et les zones se recouvrent, une seule compte donc. Les lignes sans label `zone` sont gardées.                   |
| `service_mappings`     | table  | `{}`          | Associe les noms de service Perf Sentinel à la valeur du label Kepler identifiant la même charge (nom de conteneur pour `container`, nom de commande processus pour `process`). Les services associés à une même valeur se partagent ses joules selon leurs opérations. |
| `auth_header`          | chaîne | *(aucun)*     | En-tête `"Name: Value"` optionnel. Préférer la variable d'environnement `PERF_SENTINEL_KEPLER_AUTH_HEADER`.                                                                     |

```toml
[green.kepler]
endpoint = "http://kepler.kepler.svc.cluster.local:28282/metrics"
scrape_interval_secs = 5
metric_kind = "container"

[green.kepler.service_mappings]
"order-svc" = "order-svc-deployment"
"chat-svc" = "chat"
```

**Ignoré en mode batch `analyze`.** Comme Scaphandre, seul `watch` lance le scraper.

**Les compteurs partageant une valeur de label sont sommés, les zones non.** Un même nom de conteneur répété entre pods (ou un même `comm` partagé par plusieurs processus) produit plusieurs séries cumulatives sous une même valeur de mapping. Leurs compteurs sont sommés avant le calcul du delta par fenêtre, le coefficient les couvre donc ensemble. Plusieurs services associés à une même valeur lisent cette seule somme : son delta est compté une fois et réparti entre eux selon les opérations que chacun a émises dans la fenêtre. Kepler expose aussi une série par zone, et les zones se recouvrent : `package` contient déjà `core` et `uncore`. Seules les lignes dont le label `zone` vaut `zone` sont lues, comme le conseille la [documentation des métriques de Kepler](https://github.com/sustainable-computing-io/kepler/blob/v0.12.0/docs/user/metrics.md#energy-zones). Les lignes sans label `zone` sont gardées. Là où Kepler lit hwmon, les zones portent le nom des capteurs : réglez `zone` sur l'une des valeurs exposées, sinon l'avertissement de zéro échantillon se déclenche.

**Précédence par rapport à Alumet et Scaphandre.** Les deux surclassent `kepler_ebpf` pour un même service. Tous trois lisent RAPL. Kepler attribue à chaque charge une part de la puissance active seule, au prorata de son temps CPU. Kepler est surtout utile sur Kubernetes, où il tourne en `DaemonSet` et étiquette ses séries par conteneur. Aucune image Kepler arm64 n'est publiée à la v0.12.0, et une VM sans RAPL n'obtient aucune lecture Kepler. Voir [docs/FR/LIMITATIONS-FR.md](LIMITATIONS-FR.md#limites-de-précision-kepler) pour les exigences de plateforme et les limites de précision.

**Forme de déploiement en production.** Kepler s'exécute en général comme `DaemonSet` Kubernetes, un pod par nœud. Le scraper actuel effectue un GET direct et la réponse doit exposer les séries Kepler elles-mêmes. L'endpoint `/metrics` d'un serveur Prometheus expose les métriques internes de Prometheus, pas les séries qu'il a scrapées. L'endpoint d'exemple est le Service des manifestes amont (namespace `kepler`, port 28282). Ce Service ClusterIP envoie chaque scrape vers un pod Kepler arbitraire, il ne couvre donc tout le cluster que si celui-ci n'a qu'un nœud. Pour un cluster multi-nœuds, exécutez un Perf Sentinel par nœud ou fournissez un endpoint de fédération/proxy qui expose directement les séries Kepler agrégées au format d'exposition Prometheus. Le mode de requête PromQL natif est réservé à une version ultérieure.

#### `[green.alumet]` (optionnel, opt-in)

Intégration opt-in avec [Alumet](https://github.com/alumet-dev/alumet) (INRIA/LIG, EUPL-1.2) pour l'énergie mesurée. Alumet est un framework de mesure modulaire : un plugin source produit des relevés, des plugins de transformation optionnels les attribuent aux charges de travail, et un plugin de sortie les expose. Perf Sentinel scrape la sortie `prometheus-exporter` et ne prend en charge que des séries issues de la source `rapl` (voir plus bas). Une fois configuré, le daemon `watch` publie un coefficient mesuré par opération étiqueté `alumet_rapl`, qui **surclasse toutes les autres sources mesurées**, Scaphandre compris.

| Champ                  | Type     | Défaut    | Description                                                                                                                                                                                |
|------------------------|----------|-----------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `endpoint`             | chaîne   | *(aucun)* | URL complète de l'endpoint `/metrics` du `prometheus-exporter` d'Alumet (port amont par défaut 9091). Requis si la section est présente                                                    |
| `scrape_interval_secs` | entier   | `5`       | Fréquence de scrape, en secondes. Plage valide : 1-3600                                                                                                                                    |
| `metric_name`          | chaîne   | *(aucun)* | Nom de la métrique Prometheus **exactement tel qu'il apparaît sur le fil**, `prefix`/`suffix` de l'exporteur inclus. Requis, sans défaut (voir plus bas)                                   |
| `label_key`            | chaîne   | *(aucun)* | Label Prometheus portant l'identité de la charge de travail. Requis, sans défaut. `name` pour la source `k8s` (nom du pod), `domain` pour une série RAPL brute                             |
| `energy_interval_secs` | flottant | `1.0`     | Durée réelle (temps d'horloge), en secondes, que couvre la valeur en joules scrapée. **Doit correspondre au `poll_interval` de la source Alumet** qui alimente la métrique (voir plus bas) |
| `service_mappings`     | table    | `{}`      | Associe les noms de service Perf Sentinel à la valeur de label Alumet identifiant la même charge de travail. Les services associés à une même valeur se partagent son énergie selon leurs opérations |
| `auth_header`          | chaîne   | *(aucun)* | En-tête `"Name: Value"` optionnel. Préférer la variable d'environnement `PERF_SENTINEL_ALUMET_AUTH_HEADER`                                                                                 |

**Pourquoi `metric_name` et `label_key` n'ont pas de défaut.** L'exporteur d'Alumet ajoute un `prefix` en tête et un `suffix` en fin (par défaut `_alumet`) à chaque nom de métrique, et la série par service est produite par une formule `energy-attribution` dont *vous* choisissez le nom. Aucun défaut ne serait correct pour tous les déploiements, et une mauvaise supposition ne scraperait rien. Lisez les noms sur votre propre endpoint :

```bash
curl -s http://localhost:9091/metrics | grep -i energy
```

**`metric_name` doit désigner une série RAPL en joules.** Le scraper lit chaque valeur comme les joules consommés pendant un `energy_interval_secs` et l'étiquette `alumet_rapl`. Pointez-le vers `rapl_consumed_energy` lui-même, ou vers une formule `energy-attribution` alimentée par `rapl`, comme dans l'exemple plus bas. Les autres sources d'énergie d'Alumet ne sont pas prises en charge. Les séries d'énergie de `nvidia-nvml`, `amd-gpu` et `grace-hopper` sont en millijoules et la jauge `input_power` de `nvidia-jetson` est une puissance en milliwatts, elles arriveraient donc dans la mauvaise unité, 1000 fois trop haut pour des millijoules. `energy-estimation-tdp` publie des joules, mais c'est une estimation tirée du TDP du processeur, pas une mesure. Scrapée ici, chacune de ces séries serait publiée comme du RAPL mesuré sous `alumet_rapl`. Au démarrage, le scraper journalise un avertissement quand `metric_name` ressemble à l'une de ces séries ou se termine par une autre unité que le joule.

**Plusieurs lignes par service sont sommées.** Le `label_key` d'Alumet est couramment partagé : un pod porte une ligne par domaine RAPL (`package` + `dram`), et `label_key = "domain"` sur un hôte bi-socket porte une ligne `domain="package"` par socket. Toutes les lignes partageant une valeur de label sont sommées, ce qui est la lecture physiquement correcte puisque l'énergie est additive (les lignes NaN ou négatives sont ignorées). Deux conséquences à configurer. Choisissez `label_key` de sorte que les lignes partageant une valeur soient bien celles que vous voulez additionner. Et assurez-vous que ces lignes ne se recouvrent pas : les domaines RAPL s'emboîtent (`psys` contient `package`, `package` contient `pp0`/`pp1`), donc une formule qui émet un domaine parent et son enfant pour un même label compte deux fois la part partagée. `package` plus `dram` se somme correctement, `psys` plus `package` non. Plusieurs services associés à une même valeur, par exemple tous les services associés à `package` avec `label_key = "domain"`, lisent cette seule somme et se la répartissent selon les opérations que chacun a émises dans la fenêtre, donc la lecture est comptée une fois.

**Pourquoi `energy_interval_secs` existe.** C'est le champ à ne pas rater. L'exporteur d'Alumet publie chaque mesure comme une **jauge Prometheus contenant la dernière valeur émise**, et `rapl_consumed_energy` est un `CounterDiff` : les joules brûlés pendant un `poll_interval` de la source, ni un compteur cumulatif ni une puissance. Perf Sentinel divise par cet intervalle pour retrouver des watts. L'intervalle n'apparaît nulle part sur le fil, il doit donc être déclaré ici et correspondre à la configuration côté Alumet. **Un écart met l'énergie et le carbone à une échelle linéairement fausse, en silence** : déclarer `1.0` alors qu'Alumet échantillonne à `5s` surestime l'énergie d'un facteur 5, sans aucun avertissement. Voir [docs/FR/LIMITATIONS-FR.md](LIMITATIONS-FR.md#limites-de-précision-alumet). Le daemon affiche la valeur utilisée dans la ligne de log `Alumet scraper started`.

**Configuration Alumet correspondante.** L'attribution par service demande trois plugins Alumet qui travaillent ensemble, `rapl` seul ne mesure que la machine entière et `procfs` n'identifie les processus que par PID :

```toml
# alumet-config.toml
[plugins.rapl]
poll_interval = "1s"          # <- energy_interval_secs de perf-sentinel doit valoir ceci

[plugins.k8s]
# découverte des pods, fournit les attributs `name` / `namespace`

[plugins.energy-attribution.formulas.attributed_energy_cpu]
expr = "cpu_energy * cpu_usage / 100.0"
ref = "cpu_energy"
retention_time = "10s"        # requis, au moins deux fois le poll_interval d'entrée le plus lent (k8s : 5s par défaut)

[plugins.energy-attribution.formulas.attributed_energy_cpu.per_resource]
cpu_energy = { metric = "rapl_consumed_energy", resource_kind = "local_machine", domain = "package_total" }

[plugins.energy-attribution.formulas.attributed_energy_cpu.per_consumer]
cpu_usage = { metric = "cpu_percent", kind = "total" }

[plugins.prometheus-exporter]
port = 9091
suffix = "_alumet"            # <- d'où le nom de métrique en _alumet ci-dessous
```

Le côté Perf Sentinel correspondant :

```toml
[green.alumet]
endpoint = "http://localhost:9091/metrics"
scrape_interval_secs = 5
metric_name = "attributed_energy_cpu_alumet"
label_key = "name"
energy_interval_secs = 1.0

[green.alumet.service_mappings]
"order-svc" = "order-svc-pod"
"chat-svc" = "chat-svc-pod"
```

**Ignoré en mode batch `analyze`.** Comme tous les backends d'énergie mesurée, seul `watch` lance le scraper.

**Précédence.** `alumet_rapl` est en tête de la chaîne mesurée, devant `scaphandre_rapl`. Les deux lisent RAPL, mais l'échantillonnage d'Alumet est mesurablement moins erroné et il attribue par cgroup plutôt que par processus. Faire tourner les deux sur le même service est pris en charge, et Alumet l'emporte.

**Pièges du packaging amont.** Le `.deb` amont installe `/etc/alumet/alumet-config.toml` (avec des sections csv, procfs, perf, ...) et son script d'encapsulation `alumet-agent` pointe `ALUMET_CONFIG` dessus si la variable n'est pas déjà définie. Activer prometheus-exporter via `--plugins` fonctionne même si ce fichier n'a pas de section `prometheus-exporter`. L'agent remplit la section absente à partir des défauts du plugin (`prefix ""`, `suffix "_alumet"`, port 9091). Pour une config contrôlée plutôt que celle livrée, pointez `ALUMET_CONFIG` vers un chemin vierge pour que l'agent régénère les défauts de votre jeu de plugins, ou lancez `config regen`. En conteneur, le binaire packagé porte des file capabilities (`cap_perfmon`, `cap_sys_nice`, `cap_sys_ptrace`) : un `docker run` nu échoue en EPERM sans les `--cap-add` correspondants.

**Le scraper seul ne met pas `alumet_rapl` dans le rapport.** Déclarer `[green.alumet]` démarre le scraper (visible sur `/api/energy`), mais le coefficient mesuré n'atteint `green_summary` que quand le scoring green résout une région pour les spans (`[green] default_region`, `[green.service_regions]`, ou un attribut de span `cloud.region`). Sans cela, `per_service_energy_model` continue d'afficher le tag proxy, ce qui se lit facilement comme une intégration Alumet cassée.

**Alumet est pré-1.0** (v0.9.5 au moment de l'écriture). Les noms de métriques et la configuration des plugins peuvent changer d'une version à l'autre. Si un scrape cesse de correspondre après une montée de version d'Alumet, le daemon avertit avec `no samples matched the configured metric` après trois ticks consécutifs. La branche de développement d'Alumet, après la v0.9.5, ajoute l'unité à chaque nom de métrique qui en a une : `attributed_energy_cpu_alumet` s'y lit `attributed_energy_cpu_alumet_joules`. En passant à une version qui porte ce changement, mettez `metric_name` à jour en conséquence.

##### `[green.alumet.database]` (optionnel)

Déclare une charge de base de données mesurée par Alumet. Une base n'émet pas de spans, elle ne peut donc jamais apparaître dans `service_mappings` (zéro op, le chemin des coefficients par op l'ignore). À la place, son énergie sur chaque fenêtre de scoring est multipliée par le ratio de gaspillage SQL (`avoidable_sql_io_ops / total_sql_io_ops`) et publiée dans `green_summary.database_waste`. Cette énergie d'hôte est comptée dans `energy_kwh` et `co2.total` (en `co2.database_gco2`) dès que son carbone est converti à la région ci-dessous, et le chiffre porte alors `in_totals = true`. Les chiffres propres des services ne la comprennent jamais. La divulgation la publie dans le bloc séparé `aggregate.database_waste`, une ventilation de ce que les totaux contiennent déjà. Sans cette section, le bloc porte la forme estimée, la part côté application des spans SQL, déjà comprise dans les totaux. Voir `docs/FR/METHODOLOGY-FR.md` pour la formule et [Limites de précision Alumet](LIMITATIONS-FR.md#limites-de-précision-alumet) pour la raison d'une borne basse.

```toml
[green.alumet.database]
label_value = "postgres-pod"   # valeur portée par label_key pour le cgroup de la base, telle quelle
region = "eu-west-3"           # sinon [green] default_region, l'un des deux est requis
```

`label_value` est obligatoire, apparié exactement comme une valeur de `service_mappings`. L'énergie de l'hôte de la base a besoin d'une région pour être convertie en gCO2, avec les mêmes identifiants de région que `[green.service_regions]` : ce `region`, celui de `[green.database_static]`, ou `[green] default_region`. Sans aucun d'eux, la config ne se charge pas, et quand cette section et `[green.database_static]` déclarent toutes deux une région, elles doivent concorder. C'est la région qui permet à l'énergie d'entrer dans les totaux : un chiffre dont le carbone n'est pas converti reste hors d'eux. Une base par configuration, déclarez le cgroup qui sert votre trafic SQL. Exige un agent sur l'hôte de la base, donc inapplicable à une base managée. Pour un cluster provisionné, voir `[green.database_static]`.

**Périmètre.** La charge doit tourner sur des hôtes qu'aucune lecture applicative au niveau du nœud ne couvre. Sous `label_key = "domain"` avec des `service_mappings` non vides, la section est refusée : un domaine RAPL couvre un nœud entier, la base et les services qui y tournent compteraient donc les mêmes joules. Mesurez par cgroup avec un autre `label_key`, ou retirez l'un des deux. À côté de `[green.cloud.services]` ou de `[green.redfish] service_mappings` non vides, la section exige aussi `[green.database_static]`. Si la base tourne sur des hôtes que ces backends lisent, retirez cette section, son énergie est déjà dans leurs lectures. Si elle tourne sur des hôtes à part, ajoutez `[green.database_static]` avec `placement = "dedicated"`. Un service cloud laissé sur la requête CPU par défaut de son fournisseur émet un avertissement au chargement, car cette requête renvoie la première série de la flotte, qui peut être l'hôte de la base.

##### `[green.alumet.broker]` (optionnel)

Le jumeau messaging de la section ci-dessus. Déclare un broker de messages mesuré par Alumet : un broker n'émet pas de spans propres non plus, donc son énergie de fenêtre est multipliée par le ratio de gaspillage messaging (`avoidable_messaging_io_ops / total_messaging_io_ops`) et rapportée en `green_summary.messaging_waste`, une énergie d'hôte comptée dans les totaux (en `co2.messaging_gco2`) selon la même règle et publiée dans la divulgation en `aggregate.messaging_waste`.

```toml
[green.alumet.broker]
label_value = "kafka-pod"      # valeur portée par label_key pour le cgroup du broker, telle quelle
region = "eu-west-3"           # sinon [green] default_region, l'un des deux est requis
```

L'énergie de l'hôte du broker a besoin d'une région pour être convertie en gCO2 : ce `region`, celui de `[green.broker_static]`, ou `[green] default_region`. Sans aucun d'eux, la config ne se charge pas, et quand cette section et `[green.broker_static]` déclarent toutes deux une région, elles doivent concorder. Les règles de périmètre de `[green.alumet.database]` s'appliquent aussi, avec `[green.broker_static]` comme section qui place le broker sur des hôtes à part.

Un même cgroup ne peut pas alimenter deux chiffres : un `label_value` qui apparaît aussi dans `service_mappings`, ou qui correspond à la déclaration de base de données, est rejeté au chargement de la config. Exige un agent sur l'hôte du broker, donc inapplicable à un broker managé. Pour un cluster provisionné, voir `[green.broker_static]`.

#### `[green.database_static]` (optionnel, opt-in)

Déclare les hôtes de la base de données qu'interrogent les services tracés, sans agent ni métrique. C'est la voie d'une base provisionnée avec une classe d'instance typée et aucun hôte instrumentable, comme un cluster RDS ou Aurora provisionné ou un serveur flexible Azure, ou des VM autogérées sans agent. Les bases serverless ou facturées en unités de capacité n'ont aucun nœud provisionné et ne peuvent pas être déclarées.

```toml
[green.database_static]
placement = "dedicated"        # requis dès qu'une clé est posée : "dedicated" ou "shared"
nodes = 2                      # hôtes de la base, lecteurs et standbys provisionnés compris, de 1 à 10000
instance_type = "db.r6i.large" # une ligne embarquée ou une classe managée qui s'y résout
region = "eu-west-3"           # sinon la région de [green.alumet.database], sinon [green] default_region
```

`placement` dit où tourne la base, et chaque autre clé est vérifiée contre lui au chargement, pour toutes les sous-commandes, `analyze` compris. Une section sans `placement` échoue avec un message qui nomme les deux placements. La section ne prend pas de `provider`, le type d'instance suffit à chiffrer les hôtes.

- **`dedicated`** : des hôtes à elle, sur lesquels ne tourne aucun service tracé. `nodes` et `instance_type` vont ensemble. Les deux ne peuvent être omis qu'à côté de `[green.alumet.database]` et sans `cpu_query`, la forme d'une base mesurée par Alumet qui déclare son placement. `shared_with` est refusé.
- **`shared`** : un pod de base de données sur un nœud applicatif. `cpu_query` est requis (le CPU du pod en pourcentage de ce nœud), `nodes` est absent ou vaut 1, et `instance_type` est requis quand `shared_with` (les entrées de `[green.cloud.services]` qui lisent ce nœud) est vide. Une déclaration partagée n'a pas de plafond : sa requête CPU facture la part du pod dans le nœud, voir **Une base sur un nœud applicatif** plus bas. Une base répartie sur plusieurs nœuds applicatifs ne peut pas être déclarée partagée, et une déclaration partagée est refusée à côté de `[green.alumet.database]`.

`nodes` compte chaque hôte provisionné du cluster : l'instance d'écriture, chaque lecteur et chaque standby, un standby Multi-AZ compris. Un standby ne sert aucune requête mais consomme de l'énergie, l'omettre sous-compte donc le cluster. `instance_type` fait de 1 à 256 caractères sans caractère de contrôle, et accepte les lignes embarquées et les noms de classes managées listés dans [Classes de services managés](./INSTANCE-TYPES-FR.md#classes-de-services-managés) : `db.r6i.large` se chiffre comme `r6i.large`, `db.m5d.large` comme `m5.large` et un `Standard_D4ds_v5` Azure comme `Standard_D4s_v5`. Les offres Cloud SQL `db-n1-standard-N` et `db-n1-highmem-N` se chiffrent comme leur ligne `n1`, `db-perf-optimized-N-V` d'Enterprise Plus comme `n2-highmem-V`, et les objectifs vCore Azure SQL de série standard (`GP_Gen5_N`, `BC_Gen5_N`, `HS_Gen5_N`, serverless `GP_S_Gen5_N` et `HS_S_Gen5_N` à leur maximum de vCores) à N vCores de la ligne de référence Skylake. Les noms respectent la casse. Un type qui ne se résout sur aucune ligne fait échouer le chargement, en nommant le type, la graphie qui se résout quand seule la casse diffère, et la raison quand il s'agit d'une classe laissée de côté exprès (`db.serverless`, les offres Cloud SQL à cœur partagé et à cœur dédié personnalisées, les objectifs Azure SQL Fsv2 retirés). L'énergie de l'hôte de la base a besoin d'une région : celle de cette section, sinon celle de `[green.alumet.database]`, sinon `[green] default_region`. Sans aucune, le chargement échoue, et deux régions déclarées qui diffèrent sont refusées.

Une déclaration dédiée avec `nodes` et `instance_type` a un plafond. Sans `cpu_query`, l'énergie vaut `nodes × max_watts × durée de la fenêtre`, le plafond `E(n) = n × P_max` de `[green.broker_static]` ci-dessous, avec les trois mêmes propriétés : il borne le calcul et non la consommation murale, il compte les hôtes provisionnés et non leur charge, et il ne bouge pas quand l'application corrige un N+1. Il est rapporté en `green_summary.database_waste` avec `model = "database_specpower"` et entre dans les totaux comme un chiffre mesuré, en `co2.database_gco2`, converti à la région ci-dessus. La divulgation compte ses fenêtres comme déclarées, jamais comme mesurées. Ce que le plafond laisse de côté sur une base est dans [docs/LIMITATIONS-FR.md](LIMITATIONS-FR.md#limites-de-précision-des-bases-déclarées).

**Des hôtes chiffrés sur leur CPU.** Une déclaration dédiée peut aussi poser `cpu_query`, une requête PromQL qui renvoie le CPU moyen des hôtes déclarés en pourcentage, en une seule série. Elle demande `[green.cloud]` : le scraper cloud l'exécute contre `prometheus_endpoint`, avec le même en-tête d'authentification et le même intervalle, et facture `nodes × (idle_watts + (max_watts - idle_watts) × cpu / 100)` sur chaque intervalle, étiqueté `cloud_specpower`. Pour une base managée lue via cloudwatch_exporter :

```toml
[green.database_static]
placement = "dedicated"
nodes = 2
instance_type = "db.r6i.large"
cpu_query = 'avg(aws_rds_cpuutilization_average{dbinstance_identifier=~"orders-db-.*"})'
```

Une réponse à plusieurs séries, un résultat vide ou non fini, ou un échec de récupération ne facture rien sur ce tick, avertit une fois jusqu'à la prochaine lecture valide, et laisse grimper `perf_sentinel_database_cpu_last_scrape_age_seconds`. Quand aucune lecture n'est arrivée depuis trois intervalles de scrape, le plafond ci-dessus facture la période en `database_specpower`, et la première lecture après le retour, qui remonte sur cette période, est écartée. La requête ne peut être ni une requête CPU par défaut d'un fournisseur, ni `[green.cloud] cpu_metric`, ni la requête d'une entrée de `[green.cloud.services]`, qui lisent des nœuds applicatifs, et elle ne peut pas côtoyer `[green.alumet.database]`. Les métriques CPU managées sont en retard sur les hôtes, voir [docs/LIMITATIONS-FR.md](LIMITATIONS-FR.md#limites-de-précision-des-bases-déclarées).

**Des hôtes de tailles différentes.** Un seul type d'instance et une seule `cpu_query` ne décrivent pas un writer et un reader plus petit, ni un cluster par tenant. Une déclaration dédiée liste alors ses groupes d'hôtes identiques en entrées `[[green.database_static.hosts]]`, chacune avec ses propres `nodes`, `instance_type` et `cpu_query` facultative, et retire ces trois clés de la section elle-même :

```toml
[green.database_static]
placement = "dedicated"
region = "eu-west-3"

[[green.database_static.hosts]]
nodes = 1
instance_type = "db.r6i.xlarge"
cpu_query = 'avg(aws_rds_cpuutilization_average{dbinstance_identifier="orders-writer"})'

[[green.database_static.hosts]]
nodes = 2
instance_type = "db.r6i.large"
cpu_query = 'avg(aws_rds_cpuutilization_average{dbinstance_identifier=~"orders-reader-.*"})'
```

Chaque groupe est une déclaration à part entière. Il fixe à la fois `nodes` et `instance_type`, puisque l'exception Alumet ci-dessus ne s'applique pas aux groupes, et sa `cpu_query` suit les règles du paragraphe précédent. Il facture, se replie sur son plafond et reprend seul : un groupe dont la requête ne répond plus facture son plafond pendant que les autres gardent leurs lectures. Un groupe sans `cpu_query` facture son plafond. Le chiffre de la base est leur somme, étiquetée `cloud_specpower` tant que chaque groupe facture sur son CPU et `database_specpower` dès qu'un groupe facture son plafond, si bien qu'un repli n'est jamais publié comme une lecture. `perf_sentinel_database_cpu_last_scrape_age_seconds` lit le groupe resté le plus longtemps sans lecture. Le chargement refuse `hosts` sous `placement = "shared"` ou à côté de `[green.alumet.database]`, à côté de `nodes`, `instance_type` ou `cpu_query` au niveau de la section, avec plus de 256 entrées ou plus de 10000 nœuds au total, et deux groupes qui lisent la même `cpu_query`. La `region` de la section vaut pour chaque groupe.

**Une base sur un nœud applicatif.** Avec `placement = "shared"`, la base tourne sur un nœud qu'un groupe de requête de `[green.cloud.services]` lit déjà, son énergie est donc dans la lecture de ce nœud. `cpu_query` renvoie le CPU du pod en pourcentage de ce nœud, en une seule série : filtrée sur le nœud que lit la requête du groupe, et ramenée à un nœud en divisant le débit CPU d'un conteneur par le nombre de vCPU du nœud. `shared_with` liste exactement ce groupe : chaque service listé résout la même requête CPU, chaque service qui la résout est listé, et ils partagent une seule enveloppe de puissance, égale à celle d'`instance_type` quand il est posé (`instance_type` peut alors être omis, l'enveloppe du groupe chiffre l'hôte). À chaque tick la sonde facture la part dynamique du pod, `(max_watts - idle_watts) × cpu / 100`, plafonnée aux watts dynamiques du nœud lui-même, et retire ces mêmes watts du nœud avant que ses services se le partagent. Le nœud est compté une fois, et son idle reste aux services. Quand la base et le broker sont tous deux déclarés sur un même nœud, la base est facturée en premier et le broker au plus ce qui reste. Un tick sans lecture à une seule série de la requête du groupe ne facture rien à l'hôte, et un hôte partagé n'a pas de plafond de repli, une sonde périmée ne facture donc rien non plus. Les fenêtres qu'elle facture portent `model = "cloud_specpower"`. Pour un pod Postgres sur le nœud à 4 vCPU que lisent les services `orders` et `billing` :

```toml
[green.cloud.services]
orders = { instance_type = "m6i.xlarge", cpu_query = '100 - (avg(rate(node_cpu_seconds_total{mode="idle",instance="node-a:9100"}[5m])) * 100)' }
billing = { instance_type = "m6i.xlarge", cpu_query = '100 - (avg(rate(node_cpu_seconds_total{mode="idle",instance="node-a:9100"}[5m])) * 100)' }

[green.database_static]
placement = "shared"
cpu_query = '100 * sum(rate(container_cpu_usage_seconds_total{container="postgres",node="node-a"}[5m])) / 4'
shared_with = ["orders", "billing"]
```

Le chargement refuse un `shared_with` de plus de 256 entrées, un doublon, un nom qui n'est pas une clé de `[green.cloud.services]`, des services répartis sur plusieurs requêtes CPU, un groupe auquel il manque un de ses services, des services aux enveloppes différentes ou une enveloppe différente de celle d'`instance_type`, et un service mappé dans `[green.redfish]` ou dans `[green.alumet]` sous `label_key = "domain"` : ces backends lisent le nœud entier, l'hôte est donc déjà dans leur lecture et ne doit pas être déclaré. Un hôte partagé avec un `shared_with` vide à côté de `[green.cloud.services]`, de mappings Redfish ou de domaines Alumet se charge avec un avertissement, puisque sa part est facturée en plus de chaque lecture : ce n'est juste que si aucune lecture applicative ne contient son nœud, un nœud dont Kepler lit les services par conteneur par exemple. Une base répartie sur plusieurs nœuds applicatifs ne peut pas être déclarée partagée, un nœud est une lecture. Mesurez-la par conteneur (Kepler, un cgroup Alumet), ou déclarez-la dédiée seulement quand ses nœuds n'hébergent aucun service cloud tracé.

Daemon uniquement. Quand cette section et `[green.alumet.database]` sont toutes deux configurées, **la mesure gagne** : le plafond ne facture que les périodes où la série Alumet de la base est périmée, selon l'arbitrage que `[green.broker_static]` décrit ci-dessous. Un seul daemon déclare la base : chaque daemon qui la déclare facture le cluster entier, des daemons shardés qui portent tous la section comptent donc la base une fois par daemon.

#### `[green.broker_static]` (optionnel, opt-in)

Déclare un cluster de brokers **provisionné**, sans agent ni métrique. C'est la voie d'un cluster de brokers provisionné avec une classe d'instance typée et aucun hôte instrumentable, comme un cluster Kafka ou MQ managé, ou des VM autogérées sans agent. Les brokers serverless ou facturés en unités de capacité n'ont aucun nœud provisionné et ne peuvent pas être déclarés.

```toml
[green.broker_static]
placement = "dedicated"        # requis dès qu'une clé est posée : "dedicated" ou "shared"
nodes = 3                      # nœuds de broker provisionnés, de 1 à 10000
instance_type = "kafka.m5.large"  # une ligne embarquée ou une classe managée qui s'y résout
region = "eu-west-3"           # sinon la région de [green.alumet.broker], sinon [green] default_region
```

`placement` dit où tournent les brokers, et chaque autre clé est vérifiée contre lui au chargement, pour toutes les sous-commandes, `analyze` compris :

- **`dedicated`** : des hôtes à eux. `nodes` et `instance_type` vont ensemble. Les deux ne peuvent être omis qu'à côté de `[green.alumet.broker]` et sans `cpu_query`, la forme d'un broker mesuré par Alumet qui déclare son placement. `shared_with` est refusé.
- **`shared`** : un pod de broker sur un nœud applicatif. `cpu_query` est requis (le CPU du pod en pourcentage de ce nœud), `nodes` est absent ou vaut 1, et `instance_type` est requis quand `shared_with` (les entrées de `[green.cloud.services]` qui lisent ce nœud) est vide. Une déclaration partagée n'a pas de plafond : sa requête CPU facture la part du pod dans le nœud et la retire de la lecture du nœud, selon les règles que `[green.database_static]` donne pour une base sur un nœud applicatif. Un broker réparti sur plusieurs nœuds applicatifs ne peut pas être déclaré partagé, et une déclaration partagée est refusée à côté de `[green.alumet.broker]`.

`instance_type` accepte les lignes embarquées et les noms de classes managées listés dans [Classes de services managés](./INSTANCE-TYPES-FR.md#classes-de-services-managés) (`kafka.m5.large`, `mq.m5.large`). Les noms respectent la casse. Un type qui ne se résout sur aucune ligne fait échouer le chargement, en nommant le type, la graphie qui se résout quand seule la casse diffère, et la raison quand il s'agit d'une classe laissée de côté exprès. L'énergie de l'hôte du broker a besoin d'une région : celle de cette section, sinon celle de `[green.alumet.broker]`, sinon `[green] default_region`. Sans aucune, le chargement échoue, et deux régions déclarées qui diffèrent sont refusées. `provider` est accepté (`aws`, `gcp`, `azure`, `scaleway` ou `generic`, toute autre valeur est refusée) mais ne chiffre rien, le type d'instance seul chiffre les hôtes : le renseigner émet donc un avertissement au chargement. Une section sans `placement` échoue avec un message qui nomme les deux placements.

Une déclaration dédiée avec `nodes` et `instance_type` facture de l'énergie, une déclaration partagée aussi par sa `cpu_query`. Cette énergie entre dans les totaux comme un chiffre mesuré, en `co2.messaging_gco2`, convertie à la région ci-dessus. Une `cpu_query` sur une déclaration dédiée chiffre les brokers sur leur CPU, étiquetés `cloud_specpower` avec le plafond ci-dessous comme repli `broker_specpower`, comme le décrit `[green.database_static]` plus haut, et `perf_sentinel_broker_cpu_last_scrape_age_seconds` suit ses scrapes. Des brokers de tailles différentes prennent des groupes `[[green.broker_static.hosts]]`, selon les règles que `[green.database_static]` donne pour des hôtes de tailles différentes, avec `broker_specpower` comme étiquette déclarée.

L'énergie vaut `nodes × max_watts × durée de la fenêtre`, suivant `E(n) = n × P_max` : les nœuds provisionnés multipliés par leur plafond de puissance. Trois propriétés à accepter avant de s'appuyer dessus :

- **Le chiffre borne le calcul, pas la consommation murale.** `max_watts` est la puissance à 100 % de CPU, tirée d'une table SPECpower qui couvre le CPU et la carte mère et exclut le stockage, le réseau et les pertes d'alimentation. Or ce sont eux qui dominent sur un broker : c'est donc un plafond sur les vCPU déclarés, pas sur ce que le cluster tire de la prise. Un nœud Kafka limité par le stockage peut consommer plus que ce que ce chiffre rapporte.
- **Il compte l'infrastructure provisionnée, pas consommée.** Un cluster de trois nœuds est immobilisé qu'il tourne à 10 % ou à 60 %. Dans l'autre sens, une période sans trafic facture au plus une heure, donc un cluster majoritairement inactif est sous-compté.
- **Le chiffre ne réagit à aucun changement applicatif.** Grouper vos publications ne le fera pas bouger, puisque rien dedans ne dépend du trafic. Si vous voulez un chiffre qui répond à une remédiation, il faut la voie mesurée.

Daemon uniquement, comme toute voie mesurée, puisqu'une durée de fenêtre est nécessaire. Quand cette section et `[green.alumet.broker]` sont toutes deux configurées, **la mesure gagne**. La précédence suit la série Alumet du broker, pas l'endpoint de scrape : un scrape qui répond sans porter le `label_value` déclaré ne mesure rien, la déclaration prend donc le relais au lieu d'être supprimée par un endpoint qui fonctionne. L'énergie déjà mise de côté par la série est livrée d'abord, et le delta qui arrive à la reprise est jeté une fois, puisqu'il remonte sur du temps déjà facturé par la déclaration.

#### `[green.redfish]` (optionnel, opt-in)

Intégration opt-in avec le standard BMC [Redfish](https://www.dmtf.org/standards/redfish) pour lire la puissance du châssis des serveurs physiques. Contrairement à Scaphandre et Kepler (qui mesurent uniquement CPU + DRAM), la lecture du BMC couvre tout le châssis, ventilateurs, disques et cartes réseau compris. L'inclusion des pertes de l'alimentation dépend de l'endroit où le BMC mesure, voir [docs/FR/LIMITATIONS-FR.md](LIMITATIONS-FR.md#limites-de-précision-redfish-bmc). Serveurs physiques dont le BMC est joignable par le daemon, quelle que soit l'architecture du processeur, pas de VMs cloud.

| Champ                  | Type   | Défaut    | Description                                                                                                                                                                                                                                                                                                                                                                        |
|------------------------|--------|-----------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `endpoints`            | table  | *(vide)*  | Table `chassis_id` → table d'endpoint avec `url` + `schema`. Obligatoire pour activer le scraper. Deux identifiants qui désignent un même châssis sont rejetés au chargement : même schéma d'URL, même hôte (casse ignorée), même port (celui du schéma par défaut quand il est absent) et même chemin `/redfish/v1/Chassis/<id>`, donc `/Power` et `/EnvironmentMetrics` d'un châssis entrent en collision. |
| `scrape_interval_secs` | entier | `60`      | Fréquence de scrape par châssis. Plage valide : 15-3600 (protection contre la limitation de débit BMC).                                                                                                                                                                                                                                                                            |
| `service_mappings`     | table  | `{}`      | Associe les noms de service Perf Sentinel au châssis qui les héberge. Chaque service mappé au même châssis reçoit le même coefficient au niveau du châssis. Un avertissement au chargement signale une requête CPU cloud qui partage des services avec un châssis mais en lit d'autres qu'il ne liste pas : ceux-ci gardent une part du nœud que le châssis facture à nouveau. |
| `ca_bundle_path`       | chaîne | *(aucun)* | **Réservé à une version ultérieure.** Définir ce champ aujourd'hui empêche le scraper de démarrer avec une erreur claire. Un certificat BMC émis par une CA privée, ou un certificat auto-signé qui n'est pas un certificat de CA et dont les noms alternatifs (SAN) couvrent l'hôte de l'endpoint, est accepté dès qu'il figure dans `SSL_CERT_FILE` (voir [Proxy sortant et CA privée](#proxy-sortant-et-ca-privée)). |
| `auth_header`          | chaîne | *(aucun)* | En-tête d'authentification Basic au format curl. Préférer la variable d'environnement `PERF_SENTINEL_REDFISH_AUTH_HEADER`. L'authentification Session-token (POST `/SessionService/Sessions`) n'est pas encore prise en charge.                                                                                                                                                    |

Chaque table d'endpoint a deux champs : `url` (chaîne, URL Redfish complète chemin inclus) et `schema` (chaîne, `"legacy_power"`, `"environment_metrics"` ou `"sensor"`). Le schema sélectionne le pointeur JSON canonique utilisé par le parser, sans pointeur tapé par l'opérateur :

| `schema`              | Chemin servi par le BMC                       | Pointeur JSON lu par le parser                                                                          |
|-----------------------|-----------------------------------------------|---------------------------------------------------------------------------------------------------------|
| `legacy_power`        | `/redfish/v1/Chassis/{id}/Power`              | `/PowerControl/{n}/PowerConsumedWatts`, `{n}` l'entrée dont le `PhysicalContext` est `Chassis`, sinon 0 |
| `environment_metrics` | `/redfish/v1/Chassis/{id}/EnvironmentMetrics` | `/PowerWatts/Reading`                                                                                   |
| `sensor`              | `/redfish/v1/Chassis/{id}/Sensors/{sensorId}` | `/Reading`, si `ReadingUnits` vaut `W` et `PhysicalContext` vaut `Chassis` ou est absent                |

```toml
[green.redfish]
scrape_interval_secs = 60

[green.redfish.endpoints."chassis-legacy-1"]
url = "https://bmc-rack-01.dc.example/redfish/v1/Chassis/1/Power"
schema = "legacy_power"

[green.redfish.endpoints."chassis-modern-1"]
url = "https://bmc-rack-02.dc.example/redfish/v1/Chassis/1/EnvironmentMetrics"
schema = "environment_metrics"

[green.redfish.service_mappings]
"order-svc"  = "chassis-legacy-1"
"chat-svc"   = "chassis-legacy-1"
"ledger-svc" = "chassis-modern-1"
```

**Quel schema choisir.** La DMTF a déprécié `/Power` (legacy_power) dans la Release 2020.4 au profit de `PowerSubsystem`, et la puissance du châssis est passée dans `EnvironmentMetrics.PowerWatts` (environment_metrics). La spécification Redfish rend facultative l'implémentation d'une ressource dépréciée, la prise en charge varie donc selon le firmware : le projet amont OpenBMC ne sert plus `/Power` par défaut depuis le 2025-08-26 et prévoit de retirer l'option, et certains firmwares actuels servent `/Power` sans lecture `PowerWatts` dans `/EnvironmentMetrics`. Relevez l'identifiant du châssis dans `/redfish/v1/Chassis`, interrogez les deux ressources sur votre BMC et gardez celle qui renvoie un nombre :

```bash
curl -sS -u "$BMC_USER" https://bmc.example/redfish/v1/Chassis | jq '.Members'
curl -sS -u "$BMC_USER" https://bmc.example/redfish/v1/Chassis/1/EnvironmentMetrics | jq '.PowerWatts'
curl -sS -u "$BMC_USER" https://bmc.example/redfish/v1/Chassis/1/Power | jq '.PowerControl'
```

Quand le certificat du BMC vient d'une CA privée, ajoutez `--cacert` avec le fichier de la CA, celui auquel le daemon fait confiance via `SSL_CERT_FILE`.

Quand aucune des deux ne renvoie de nombre, pointez un endpoint `sensor` sur le capteur de puissance totale du châssis : le `DataSourceUri` de `PowerWatts` s'il est présent, sinon l'entrée de `/redfish/v1/Chassis/{id}/Sensors` dont le `ReadingType` vaut `Power` et qui couvre tout le châssis, avec un `PhysicalContext` `Chassis` ou aucun. Les capteurs de puissance de chaque alimentation, en entrée ou en sortie, et de chaque CPU rapportent aussi des `W`. Pointer l'un d'eux sous-estime le châssis, et le daemon ne le rejette que si son `PhysicalContext` désigne autre chose que `Chassis`.

Une flotte mixte se déclare en donnant à chaque châssis le schema que son firmware sert.

**Ignoré en mode batch `analyze`.** Comme Scaphandre et Kepler, seul `watch` intègre Redfish.

**Coefficient au niveau du nœud.** Chaque service mappé au même châssis reçoit le **même** coefficient. Deux services sur un même châssis n'auront jamais de coefficients mesurés distincts via Redfish. Voir [docs/FR/LIMITATIONS-FR.md](LIMITATIONS-FR.md#limites-de-précision-redfish-bmc) pour la discussion complète de ce compromis et de la variance JSON entre fournisseurs.

#### `[green.cloud]` (optionnel, opt-in)

Estimation d'énergie cloud-native via utilisation CPU% + interpolation SPECpower. Quand cette section est configurée, le daemon `watch` scrape les métriques CPU% depuis un endpoint Prometheus/VictoriaMetrics et utilise une table de correspondance embarquée (watts idle/max par type d'instance cloud) pour estimer la consommation énergétique par service. Prend en charge AWS, GCP, Azure et le matériel on-premise avec surcharge manuelle des watts.

| Champ                   | Type   | Défaut    | Description                                                                                                                                                                                     |
|-------------------------|--------|-----------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `prometheus_endpoint`   | chaîne | *(aucun)* | URL de base de l'API HTTP Prometheus (ex. `http://prometheus:9090` ou `https://prometheus:9090`). TLS pris en charge via hyper-rustls. Obligatoire.                                             |
| `scrape_interval_secs`  | entier | `15`      | Intervalle de sondage en secondes (plage : 1-3600).                                                                                                                                             |
| `default_provider`      | chaîne | *(aucun)* | Fournisseur cloud par défaut : `"aws"`, `"gcp"`, `"azure"`, `"scaleway"`. Les types d'instance Scaleway sont dérivés de son Product Catalog, voir [INSTANCE-TYPES-FR.md](INSTANCE-TYPES-FR.md). |
| `default_instance_type` | chaîne | *(aucun)* | Type d'instance de repli pour les services non mappés.                                                                                                                                          |
| `cpu_metric`            | chaîne | *(aucun)* | Requête PromQL par défaut pour l'utilisation CPU, partagée par chaque service sans `cpu_query` propre. Ces services lisent un seul nœud quand elle renvoie une série.                           |

Les entrées par service dans `[green.cloud.services]` acceptent deux formes :

**Instance cloud (recherche dans la table) :**

```toml
[green.cloud]
prometheus_endpoint = "http://prometheus:9090"
scrape_interval_secs = 15
default_provider = "aws"

[green.cloud.services]
"account-svc" = { provider = "aws", instance_type = "m7i.4xlarge", cpu_query = "aws_ec2_cpuutilization_average{instance_id=\"i-0123456789abcdef0\"}" }   # Sapphire Rapids
"api-asia" = { provider = "gcp", instance_type = "c4d-standard-8" }       # AMD Turin
"analytics" = { provider = "azure", instance_type = "Standard_D8s_v6" }   # Emerald Rapids
"ml-bench" = { provider = "aws", instance_type = "m8g.4xlarge", cpu_query = "aws_ec2_cpuutilization_average{instance_id=\"i-0fedcba9876543210\"}" }      # Graviton 4
```

La liste complète des types couverts, avec leurs watts au repos et maximum, est dans [`INSTANCE-TYPES-FR.md`](./INSTANCE-TYPES-FR.md). Familles d'instances modernes couvertes : AWS m7i/c7i/r7i, m7a/c7a/r7a, m6a/c6a, Graviton 2 m6g/c6g/r6g/t4g, Graviton 3 m7g/c7g/r7g et Graviton 4 m8g/c8g/r8g, GCP n1, c3, c3d, c4, c4d, n2d et t2a, et Azure Standard_Easv5, Standard_B (burstable, à vCPU complets), Standard_Dv6, Standard_Dadsv6, Standard_Dpsv6 (Cobalt 100) et Standard_Ev6. Une entrée bare-metal nommée d'après son CPU couvre Sierra Forest (`xeon-6780e`, watts au niveau système, suppose pleine possession de la puce).

**Classes managées.** `instance_type` accepte aussi les noms de classes managées de base de données, de cache, de broker et de recherche (`db.r6i.large`, `cache.r5.large`, `kafka.m5.large`, `mq.t3.micro`, `r5.large.search`) et les noms AWS ou Azure portant un modificateur de stockage ou de réseau (`db.m5d.large`, `c7gn.large`, `Standard_D4ds_v5`). Chacun se résout sur la ligne de la famille de VM qui l'héberge, ici comme dans `[green.database_static]` et `[green.broker_static]`. Les règles, et les classes laissées de côté avec leur raison, sont dans [Classes de services managés](./INSTANCE-TYPES-FR.md#classes-de-services-managés).

**Services sur un même nœud.** Les services qui se résolvent sur une même requête CPU (leur `cpu_query`, sinon `cpu_metric`, sinon le défaut de leur fournisseur, comparés après suppression des espaces de bord) sont lus comme un seul nœud. Quand la réponse contient une seule série et que tous ces services se résolvent sur les mêmes watts au repos et maximum, les watts du nœud sont répartis entre eux selon les opérations que chacun a émises dans l'intervalle, et le nœud n'est compté qu'une fois. Des services qui lisent une même requête avec des watts différents se chargent avec un avertissement qui les nomme et gardent un chiffre par service, chacun chiffré comme s'il possédait le nœud. Une requête qui renvoie plusieurs séries garde aussi un chiffre par service, chacun lisant la première série, et le daemon avertit une fois avec le nombre de séries et les services. Une requête agrégée sur une flotte (`avg` ou `sum` sans `by`) renvoie une seule série et se lit comme un seul nœud. Chiffrer chaque nœud demande donc une requête filtrée sur lui, par exemple une requête node_exporter filtrée par `instance` (`100 - (avg(rate(node_cpu_seconds_total{mode="idle",instance="node-a:9100"}[5m])) * 100)`) ou une métrique de supervision cloud filtrée sur le label où l'exporteur place l'identifiant d'instance (`aws_ec2_cpuutilization_average{instance_id="i-0123456789abcdef0"}` avec cloudwatch_exporter). Dans l'exemple ci-dessus, `account-svc` et `ml-bench` liraient sinon tous deux la requête AWS par défaut. Deux chaînes de requête différentes font deux nœuds, même quand elles lisent la même machine.

**Watts manuels (on-premise ou matériel personnalisé) :**

```toml
[green.cloud.services]
"my-service" = { idle_watts = 45, max_watts = 120 }
```

**Ignoré en mode `analyze` batch.** Seul le daemon `watch` lance le scraper Prometheus.

**Comportement de repli.** Si l'endpoint Prometheus est inaccessible, le daemon utilise le modèle proxy pour tous les services configurés cloud. Les types d'instance inconnus se rabattent sur un défaut au niveau du fournisseur.

**Limites de précision.** Le modèle d'interpolation SPECpower a une précision d'environ +/-30%, meilleure que le modèle proxy mais moins précise que Scaphandre RAPL. Voir `docs/FR/LIMITATIONS-FR.md` pour les détails.

#### `[green.electricity_maps]` (optionnel, opt-in)

Intensité carbone en temps réel via l'API Electricity Maps. Mode daemon uniquement.

| Champ                  | Type   | Défaut                               | Description                                                                                                                                                                                                                           |
|------------------------|--------|--------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `api_key`              | chaîne | aucun                                | Token d'authentification de l'API. Préférez la variable d'environnement `PERF_SENTINEL_EMAPS_TOKEN` par sécurité                                                                                                                      |
| `endpoint`             | chaîne | `https://api.electricitymaps.com/v4` | URL de base (`http://` ou `https://`). v3 reste accepté mais émet un avertissement de dépréciation au démarrage                                                                                                                       |
| `poll_interval_secs`   | entier | `300`                                | Intervalle de sondage en secondes (plage : 60-86400). À dimensionner selon le quota de votre offre                                                                                                                                                 |
| `emission_factor_type` | chaîne | `lifecycle`                          | Modèle de facteur d'émission. `lifecycle` (défaut) inclut les émissions amont (fabrication, transport). `direct` inclut uniquement la combustion. Certains référentiels Scope 2 préfèrent `direct` pour une comptabilité plus stricte |
| `temporal_granularity` | chaîne | `hourly`                             | Agrégation temporelle de la réponse API. `hourly` (défaut), `5_minutes` ou `15_minutes`. Les valeurs sub-horaires nécessitent un plan payant qui les expose, sinon l'API agrège silencieusement en horaire                            |

La sous-table `region_map` associe les régions cloud aux zones Electricity Maps. Les régions se comparent sans tenir compte de la casse, donc quand un jeton active la section, deux clés qui ne diffèrent que par la casse et pointent vers des zones différentes sont refusées au chargement :

```toml
[green.electricity_maps]
# Utilisez la variable d'environnement PERF_SENTINEL_EMAPS_TOKEN au lieu de api_key dans la config
poll_interval_secs = 300

[green.electricity_maps.region_map]
"eu-west-3" = "FR"
"us-east-1" = "US-NY"
"ap-northeast-1" = "JP-TK"
```

**Obsolescence :** si le dernier sondage réussi date de plus de 3x `poll_interval_secs`, le scraper se rabat sur les profils horaires embarqués.

**Offres et limites de débit :** l'offre gratuite d'Electricity Maps est réservée aux usages non commerciaux, et ses conditions comptent tout système de production comme un usage commercial. L'accès commercial se vend par signal et par zone, le signal d'intensité carbone en temps réel étant affiché à 6 000 € par zone et par an en octobre 2026. Dimensionnez `poll_interval_secs` selon le quota de votre offre : une zone sondée toutes les 3600 s consomme environ 720 requêtes par mois. Pour les régions françaises, `[green.rte_eco2mix]` ci-dessous est une alternative gratuite.

**Version d'API :** l'endpoint par défaut cible v4 depuis Perf Sentinel 0.5.11. v3 reste accepté (le schéma de réponse est identique sur `carbon-intensity/latest`), mais un avertissement de dépréciation est journalisé une fois au démarrage du daemon. Pour le faire taire, mettez `endpoint = "https://api.electricitymaps.com/v4"` explicitement. Pour rester sur v3 (par exemple pour valider A/B contre v4), laissez `endpoint = "https://api.electricitymaps.com/v3"` et acceptez l'avertissement.

**Valeurs inconnues pour `emission_factor_type` et `temporal_granularity` :** ces deux réglages utilisent un parser tolérant. Une faute de frappe ou une valeur non prise en charge (par exemple `temporal_granularity = "5min"` au lieu de `"5_minutes"`) ne rejette pas la config au chargement. La valeur est sanitisée, un `tracing::warn!` est émis, et le daemon se rabat sur le défaut. Surveillez les logs du daemon au démarrage si vous suspectez une faute de frappe, la ligne warn nommera le champ et la valeur fautifs.

**Visibilité dans les rapports (depuis Perf Sentinel 0.5.12) :** la configuration de scoring active (version d'API, modèle de facteur d'émission, granularité temporelle) est exposée à trois endroits pour qu'un auditeur Scope 2 puisse vérifier quel modèle carbone a produit les chiffres sans lire le TOML de l'opérateur.

- Le rapport JSON porte toujours `green_summary.scoring_config` quand le scoring GreenOps est actif : il enregistre les coefficients appliqués et un flag `electricity_maps`. Les champs propres à l'API ne sont significatifs que lorsque ce flag vaut `true`.
- Le dashboard HTML rend un bandeau de chips au-dessus de la table green-regions. Les valeurs par défaut (`v4`, `lifecycle`, `hourly`) apparaissent en chips neutres, les opt-ins (`direct`, `5_minutes`, `15_minutes`) en chips accent, l'ancien endpoint `v3` en chip d'avertissement, miroir de l'avertissement de dépréciation. Les infobulles natives du navigateur expliquent chaque valeur.
- La sortie `print_green_summary` du terminal ajoute une ligne `Carbon scoring: Electricity Maps v4, lifecycle, hourly` avant le détail par région.

Les chips et la ligne du terminal d'Electricity Maps sont masqués quand `[green.electricity_maps]` n'est pas configuré.

#### Flux des gestionnaires de réseau : `[green.rte_eco2mix]`, `[green.neso]`, `[green.fingrid]`, `[green.energinet]`, `[green.ree_esios]`, `[green.open_electricity]`, `[green.mavir]`, `[green.em6]` (optionnel, opt-in)

Intensité carbone en temps réel publiée gratuitement par les gestionnaires de réseau eux-mêmes, pour les régions cloud de leur réseau. Mode daemon uniquement. La présence d'une section active son flux. Tous comptent les émissions directes, cycle de vie exclu, ils sont donc plus bas qu'un chiffre Electricity Maps `lifecycle` pour la même heure, et ils ne mesurent pas exactement la même chose :

| Section | Gestionnaire, zone | Ce que mesure le chiffre | Pas | Clé | Licence |
|---|---|---|---|---|---|
| `[green.rte_eco2mix]` | RTE (`éCO2mix` sur ODRÉ), France | Émissions directes de la production française, imports exclus | 15 min | aucune | Licence Ouverte v2.0 |
| `[green.neso]` | NESO Carbon Intensity API, Grande-Bretagne (hors Irlande du Nord) | CO2 direct de la production plus les imports, par kWh consommé | 30 min | aucune | CC BY 4.0 et conditions d'utilisation de l'API |
| `[green.fingrid]` | Fingrid, Finlande | Émissions directes de la production (jeu 266, la base de RTE) ou de la consommation (265, imports à facteurs annuels fixes) | 3 min | gratuite | CC BY 4.0 |
| `[green.energinet]` | Energinet Energi Data Service, Danemark `DK1` et `DK2` | CO2 direct par kWh consommé, imports compris | 5 min | aucune | CC BY 4.0 |
| `[green.ree_esios]` | REE e·sios, Espagne péninsulaire (hors Baléares et Canaries) | CO2 direct de la production péninsulaire, imports exclus | 5 min | jeton gratuit | avis légal e·sios : pas d'usage commercial, crédit à Red Eléctrica |
| `[green.open_electricity]` | Open Electricity (The Superpower Institute), Australie : régions du NEM `AU-NSW`, `AU-QLD`, `AU-SA`, `AU-TAS`, `AU-VIC` et WEM `AU-WA` | Émissions directes rapportées à la production régionale, solaire en toiture compris, imports exclus | 5 min | facultative, gratuite | CC BY-NC 4.0 |
| `[green.mavir]` | MAVIR, Hongrie | CO2 direct des centrales nationales par production nationale brute, imports exclus | 15 min | aucune | pas de licence ouverte |
| `[green.em6]` | em6 (Energy Market Services, Transpower), Nouvelle-Zélande | CO2 direct de la production raccordée au réseau | 30 min | aucune | tous droits réservés |

Champs communs :

| Champ                | Type    | Défaut | Description                                                                                            |
|----------------------|---------|--------|--------------------------------------------------------------------------------------------------------|
| `regions`            | liste   | aucun  | Régions cloud desservies par le réseau, pour tous les flux sauf `energinet` et `open_electricity`. Chacune reçoit la valeur nationale |
| `region_map`         | table   | aucun  | Région cloud vers zone. `[green.energinet]` : `DK1` (ouest) ou `DK2` (est). `[green.open_electricity]` : `AU-NSW`, `AU-QLD`, `AU-SA`, `AU-TAS`, `AU-VIC` ou `AU-WA` |
| `endpoint`           | chaîne  | par flux | URL de base de l'API, `http://` ou `https://`                                                        |
| `poll_interval_secs` | entier  | 900 (RTE), 600 (Open Electricity), 300 (NESO, Energinet, e·sios, MAVIR, em6), 180 (Fingrid) | Intervalle de sondage (plage : 60-86400), proche du pas de chaque gestionnaire |
| `dataset`            | chaîne  | `"production"` | `[green.fingrid]` seulement : `"production"` (266) ou `"consumption"` (265)                     |
| `api_key`            | chaîne  | aucun  | Requise par `[green.fingrid]` et `[green.ree_esios]`, facultative pour `[green.open_electricity]`. Préférez les variables d'environnement `PERF_SENTINEL_FINGRID_API_KEY`, `PERF_SENTINEL_REE_ESIOS_API_KEY` et `PERF_SENTINEL_OPEN_ELECTRICITY_API_KEY` |

```toml
[green.rte_eco2mix]
regions = ["eu-west-3"]

[green.neso]
regions = ["eu-west-2", "uksouth"]

[green.fingrid]                  # clé dans PERF_SENTINEL_FINGRID_API_KEY
regions = ["europe-north1"]

[green.energinet]
region_map = { "dk-west" = "DK1", "dk-east" = "DK2" }

[green.ree_esios]                # jeton dans PERF_SENTINEL_REE_ESIOS_API_KEY
regions = ["eu-south-2"]

[green.open_electricity]
region_map = { "ap-southeast-2" = "AU-NSW", "ap-southeast-4" = "AU-VIC" }

[green.mavir]
regions = ["hu-central"]

[green.em6]
regions = ["ap-southeast-6"]
```

**Précédence.** Electricity Maps l'emporte pour toute région qu'il couvre aussi. Une région cloud ne peut figurer que dans un seul flux de gestionnaire, sinon le chargement échoue. Une ligne temps réel venant d'un flux de gestionnaire porte `intensity_provider` (`rte_eco2mix`, `neso`, `fingrid`, `energinet`, `ree_esios`, `open_electricity`, `mavir`, `em6`) dans `green_summary.regions[]`, absent pour Electricity Maps afin que les rapports antérieurs gardent leur forme. Le tag de modèle vaut `grid_operator_api` quand aucune région n'a utilisé Electricity Maps. Le dashboard HTML ajoute un chip par flux au bandeau du scoring carbone, et le terminal une ligne `Carbon scoring: grid operator open data (...)`, dès qu'une région l'a utilisé : une section configurée ne prouve rien à elle seule, puisque le mode batch ne lance aucun scraper.

**Fraîcheur.** Chaque gestionnaire a son propre retard. Le scraper rejette un dernier enregistrement de plus de 3 heures (RTE, Open Electricity), 2 heures (NESO, em6) ou 1 heure (Fingrid, Energinet, e·sios, MAVIR) comme flux arrêté, et un enregistrement daté de plus de 15 minutes dans le futur comme pas encore mesuré. Après 3x `poll_interval_secs` sans valeur fraîche, le scoring retombe sur les profils embarqués, comme avec Electricity Maps. Les échecs sont comptés par zone, donc un `DK2` muet est signalé même quand `DK1` répond : un avertissement nomme le flux et la zone après 3 échecs consécutifs.

**Divulgations.** Déclarez le flux dans l'org-config, `carbon_intensity_source = "rte_eco2mix"`, `"neso"`, `"fingrid"`, `"energinet"`, `"ree_esios"`, `"open_electricity"`, `"mavir"` ou `"em6"` (ou `"mixed"` pour plusieurs), pour qu'un lecteur ne confonde pas leur base avec un chiffre Electricity Maps en cycle de vie. CC BY 4.0, CC BY-NC 4.0, la Licence Ouverte et l'avis e·sios demandent une attribution là où les chiffres sont publiés : le rapport HTML nomme le gestionnaire et pointe vers lui sur chaque ligne avec le crédit que ses conditions demandent, une divulgation publiée devrait le créditer aussi.

**Conditions restrictives.** e·sios, Open Electricity, MAVIR et em6 publient gratuitement mais n'accordent pas de licence ouverte pour tout usage, leurs sections restent donc opt-in comme les autres et sont documentées avec leurs conditions :

- **REE e·sios :** l'avis légal d'e·sios, qui couvre `api.esios.ree.es`, indique "Se prohíbe expresamente la utilización de estos datos y servicios con fines comerciales". La réutilisation suit la licence type de l'article 4.2 b de la loi espagnole 37/2007 : citer Red Eléctrica comme source, ne pas dénaturer le sens, donner la date de dernière mise à jour, ne jamais laisser entendre que REE soutient la réutilisation, et n'utiliser aucun logo REE ou e·sios.
- **Open Electricity :** les données des offres Community, Academic et Pro sont sous CC BY-NC 4.0, à créditer par "Data sourced from Open Electricity (The Superpower Institute), licensed under CC BY-NC 4.0.". L'usage commercial demande l'offre Enterprise, et la page de licence range l'usage interne par un organisme à but lucratif dans le cadre de son activité parmi les usages que la licence standard ne couvre pas. Les clés doivent rester secrètes.
- **MAVIR :** pas de licence ouverte. L'avis légal ("Jogi nyilatkozat") autorise l'utilisation des actualités et informations du site dans le cadre légal applicable, et réserve à l'accord de MAVIR l'usage public ou commercial des autres contenus.
- **em6 :** l'endpoint gratuit d'intensité courante n'est accompagné d'aucune licence : Transpower New Zealand Limited se réserve tous les droits, et ses conditions n'accordent ni n'interdisent la réutilisation.

Perf Sentinel est un logiciel AGPL-3.0, et les divulgations qu'il produit sont publiques, déclaratives et non monnayées, publiées pour réduire l'empreinte carbone des applications web : le projet considère cet usage comme non commercial. Chaque gestionnaire applique néanmoins ses propres conditions et sa propre définition de l'usage commercial : confrontez-les à votre déploiement avant d'activer un flux.

**Par flux :**
- **RTE `éCO2mix` :** 50 000 appels par utilisateur et par mois sur ODRÉ, les 900 s par défaut en consomment environ 2 900. `taux_co2` est une estimation que RTE remplace ensuite par des chiffres consolidés.
- **NESO :** les conditions interdisent de vendre, louer ou réhéberger l'accès à l'API, et le logo NESO demande un accord préalable : chaque daemon l'appelle pour son propre usage. Chaque sondage lit les 3 dernières heures de créneaux d'une demi-heure et retient l'estimation la plus récente (`actual`), datée par son propre créneau. NESO cesse parfois de mettre `actual` à jour et le recopie sur chaque nouveau créneau : quand il reste identique sur 4 créneaux consécutifs alors que la prévision bouge, ou quand aucun créneau n'en porte, la prévision du créneau le plus récent est utilisée à la place.
- **Fingrid :** une clé gratuite sur data.fingrid.fi (portail développeur, "Open Data starter"), envoyée dans l'en-tête `x-api-key`, jamais dans l'URL, et débarrassée des espaces autour. Sans clé la section est désactivée avec un avertissement, comme Electricity Maps sans jeton. Une clé qui n'est pas en ASCII imprimable fait échouer le chargement, et un endpoint `http://` avec une clé journalise un avertissement de transmission en clair. Le jeu utilisé est enregistré dans `scoring_config.fingrid_dataset` (266 ou 265) et nommé sur le chip HTML et la ligne du terminal, puisque les deux bases diffèrent. Limites : 10 000 appels par jour et un toutes les 2 secondes par clé.
- **Energinet :** les valeurs sont des estimations initiales qu'Energinet dit ne pas corriger en général. Son guide d'API demande aux éditeurs de ne pas interroger depuis chaque unité d'une application : gardez un daemon par site plutôt qu'un par hôte.
- **REE e·sios :** un jeton personnel gratuit, demandé depuis l'icône "Solicitud de Token" du site e·sios, envoyé dans l'en-tête `x-api-key` avec l'en-tête `Accept` de l'exemple de requête officiel (`application/json; application/vnd.esios-api-v1+json`). Sans jeton la section est désactivée avec un avertissement. Seul l'indicateur 10355, la péninsule, est lu : les îles publient sur un autre indicateur, ne rattachez donc que des régions continentales. La valeur arrive en tCO2 par MWh et est multipliée par 1000, et un 0 compte comme un point manquant. Chaque sondage demande les 3 dernières heures, ce qui évite aussi le cache du CDN qui répond aux URL connues sans vérifier le jeton. Aucune limite de débit n'est publiée : les 300 s par défaut font un appel par point de 5 minutes.
- **Open Electricity :** l'API publie les émissions (tCO2e) et l'énergie (MWh) par région et par technologie, pas d'intensité : Perf Sentinel divise les émissions par la production, sans la série nette des batteries, la charge des batteries ni le pompage, qui compteraient deux fois la même énergie. Le solaire en toiture arrive 30 à 100 minutes après le reste, chaque région est donc lue au dernier intervalle que le solaire en toiture couvre, jusqu'à une heure et demie d'âge sur le WEM. Un horodatage marque la fin de son intervalle de 5 minutes, la convention de l'AEMO. Un appel par réseau rattaché (NEM, WEM) : les 600 s par défaut font 288 appels par jour avec les deux, sous les 500 de l'offre gratuite, et la clé, si elle est définie, part dans un en-tête `Authorization: Bearer`.
- **MAVIR :** les données n'existent que sous forme d'export XLSX du graphique 1000687, lu par un lecteur ZIP minimal. La colonne est trouvée par des mots de son en-tête hongrois (`aránya`, `tény`, `nélkül` : le ratio mesuré sans le solaire domestique ni l'autoconsommation, que MAVIR remplit 25 à 40 minutes plus tard). Un en-tête reformulé est encore trouvé, et un en-tête qui ne désigne plus exactement une colonne fait échouer le flux au lieu de lire une autre colonne. L'unité n'est imprimée nulle part, ni dans l'export ni sur les axes des graphiques : les tCO2 par MWh se déduisent des propres séries d'émissions et de production de MAVIR, et un ratio hors de 0,01 à 1,5 est rejeté comme changement d'unité. Le quart d'heure en cours est lu, daté par son début, et MAVIR le révise de quelques g/kWh une fois clos. `www.mavir.hu` interdit les robots et n'est jamais appelé. L'hôte des données `rtdwweb.mavir.hu` ne publie pas de fichier robots, et le daemon l'appelle une fois par sondage sous un User-Agent qui nomme le projet.
- **em6 :** un endpoint gratuit renvoie les trois dernières périodes d'échange. La plus récente est lue, datée par le début de sa demi-heure et publiée quelques minutes après sa fin. Le chiffre ne couvre que la production raccordée au réseau, sans la production intégrée ni les batteries.

#### `[green] calibration_file` (optionnel)

Chemin vers un fichier TOML de calibration généré par `perf-sentinel calibrate`. Quand il est présent, les facteurs de calibration par service sont chargés avec la configuration et multiplient l'énergie du modèle proxy par opération. Sans effet sur l'énergie de n'importe quel backend mesuré. À côté d'une source de base ou de broker (`[green.database_static]`, `[green.broker_static]`, `[green.alumet.database]` ou `[green.alumet.broker]`), le chargement émet un avertissement : les lectures fournies à `calibrate` doivent exclure ces hôtes, dont l'énergie entre déjà dans les totaux comme un terme d'hôte, sinon elle compte deux fois.

```toml
[green]
calibration_file = ".perf-sentinel-calibration.toml"
```

**Limites de taille d'entrée pour `perf-sentinel calibrate`.** Les deux entrées sont plafonnées pour éviter une consommation mémoire incontrôlée : le fichier `--traces` est plafonné à 1 Gio (plafond batch fixe depuis 0.8.7, identique à `analyze`) et le CSV `--measured-energy` est plafonné à 64 Mio. Calibrate termine avec une erreur claire si l'un des fichiers dépasse sa limite. 64 Mio est généreux pour des milliers d'échantillons RAPL par minute. Si vous avez besoin de plus, ouvrez une issue décrivant la charge de travail.

#### `perf-sentinel tempo` (pas de section de config)

La sous-commande `tempo` s'exécute en **mode batch** (pas daemon), récupère les traces depuis l'API HTTP d'un Grafana Tempo et les passe dans le pipeline d'analyse standard. Ses réglages propres sont des flags CLI uniquement, il n'existe pas de section `[tempo]` : `--endpoint` est obligatoire, `--max-traces` vaut `100` par défaut et est borné à 1..=10000 (le plafond de lecture du client, pas celui de Tempo), aux côtés de `--trace-id`, `--service`, `--lookback`, `--from`/`--to`, `--sort` et `--auth-header`. Lancez `perf-sentinel tempo --help` pour la liste à jour. Une table `[tempo]` écrite dans le fichier de config fait échouer le chargement depuis la 0.12.0, comme toute table racine inconnue. Le fichier `--config` s'applique quand même pour le reste, les seuils et la détection en particulier, puisque les traces récupérées passent par le même pipeline.

### `[daemon]`

Paramètres du mode streaming (`perf-sentinel watch`).

| Champ                     | Type     | Défaut                      | Description                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
|---------------------------|----------|-----------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `listen_address`          | chaîne   | `"127.0.0.1"`               | Adresse IP de liaison pour les endpoints OTLP et métriques. Utilisez `127.0.0.1` pour un accès local uniquement. **Attention :** définir une adresse non-loopback expose des endpoints non authentifiés sur le réseau, utilisez un reverse proxy ou une politique réseau                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| `listen_port_http`        | entier   | `4318`                      | Port pour le récepteur OTLP HTTP et l'endpoint Prometheus `/metrics` (plage : 1-65535)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `listen_port_grpc`        | entier   | `4317`                      | Port pour le récepteur OTLP gRPC (plage : 1-65535)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `json_socket`             | chaîne   | `"/tmp/perf-sentinel.sock"` | Chemin du socket Unix pour l'ingestion d'événements JSON                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| `max_active_traces`       | entier   | `10000`                     | Nombre maximum de traces conservées en mémoire. En cas de dépassement, la trace la plus ancienne est évincée (LRU). Plage : 1 à 1 000 000                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| `trace_ttl_ms`            | entier   | `30000`                     | Durée de vie des traces en millisecondes. Une trace devient périmée quand plus aucun span n'est arrivé depuis cette durée, et le balayage qui l'évince et l'analyse tourne sur un tic à la moitié de cette valeur, donc l'échéance effective vaut cette durée plus un tic au plus. Un span qui arrive dans cet intervalle rejoint la trace périmée et la rafraîchit au lieu d'en ouvrir une neuve, ce qui compte pour qui rejoue un identifiant de trace. Plage : 100 à 3 600 000                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| `sampling_rate`           | flottant | `1.0`                       | Fraction des traces à analyser (0.0 à 1.0). Réduire en dessous de 1.0 pour diminuer la charge dans les environnements à fort trafic. Les traces sont gardées ou jetées entières sur un hachage du trace id, donc les détecteurs par trace restent corrects sur ce qui reste et les ratios comme le ratio de gaspillage I/O échantillonnent numérateur et dénominateur de la même façon. Les comptes absolus (findings, occurrences, totaux `perf_sentinel_*`) décrivent en revanche cette fraction du trafic, et un pattern présent sur une petite part du trafic peut être entièrement écarté. Sous 1.0, le daemon émet une entrée `tuning` dans `Report.warning_details` qui le signale, et `0.0`, que cette plage accepte, reçoit son propre message puisque plus aucune trace n'est analysée. Un sampling fait par un collector en amont a le même effet et ne peut pas être détecté, voir [HELM-DEPLOYMENT-FR.md](HELM-DEPLOYMENT-FR.md#sampling-du-collector-et-ce-qui-atteint-le-daemon)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `max_events_per_trace`    | entier   | `1000`                      | Plafond par trace appliqué séparément aux événements stockés (buffer circulaire), aux contextes d'endpoint entrant (endpoint et lien parent facultatif), aux destinations de consumer retenues et aux entrées d'ascendance de spans (lien parent intermédiaire et endpoint résolu facultatif). Une trace peut contenir ces quatre collections bornées. Les événements et entrées d'ascendance sont alloués progressivement et suivent leur politique de rotation respective. Plage : 1 à 100 000                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `max_payload_size`        | entier   | `16777216`                  | Taille maximale en octets d'un payload JSON unique (défaut : 16 Mio depuis 0.5.13, monté depuis 1 Mio parce qu'un snapshot daemon de `/api/export/report` dépasse déjà 1 Mio sur un cluster modeste). Plage : 1 024 à 104 857 600 (100 Mo). Le défaut se situe à la borne supérieure inclusive de la zone de confort. Depuis 0.8.7 ce plafond ne borne que les payloads réseau du daemon : les sous-commandes batch (`analyze`, `diff`, `report`, `explain`, `calibrate`, `pg-stat`, `bench`) lisent les fichiers locaux sous un plafond fixe de 1 Gio                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `environment`             | chaîne   | `"staging"`                 | Label d'environnement de déploiement. Valeurs acceptées : `"staging"` (défaut, confiance moyenne) ou `"production"` (confiance élevée). Étiquette chaque finding avec le champ `confidence` correspondant pour les consommateurs en aval (perf-lint planifié). Insensible à la casse. Toute autre valeur est rejetée au chargement de la config                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `tls_cert_path`           | chaîne   | *(absent)*                  | Chemin vers une chaîne de certificats TLS au format PEM pour les récepteurs OTLP. Quand renseigné avec `tls_key_path`, les listeners gRPC et HTTP utilisent TLS. Quand absent, les listeners utilisent TCP en clair. Chaque listener TLS borne à 128 les handshakes en vol simultanés (non configurable) et coupe les pairs qui ne terminent pas le handshake en 10 secondes                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `tls_key_path`            | chaîne   | *(absent)*                  | Chemin vers la clé privée TLS au format PEM. Doit être renseigné avec `tls_cert_path` (les deux ou aucun). Sous Unix, le daemon avertit si le fichier de clé est lisible par le groupe ou les autres                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `api_enabled`             | booléen  | `true`                      | Active les endpoints de l'API de requêtage du daemon (`/api/findings`, `/api/explain/{trace_id}`, `/api/correlations`, `/api/status`). Mettre à `false` pour désactiver l'API tout en conservant l'ingestion OTLP et `/metrics`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `read_api_key`            | chaîne   | *(absent)*                  | Clé en lecture seule pour les deux GET qu'une clé d'écriture garde, `GET /api/acks` et `GET /api/incidents` (depuis 0.20.0). Ne satisfait jamais un `POST` ni un `DELETE`, n'ajoute jamais de porte là où aucune clé d'écriture n'est posée sauf si `require_read_key` est actif, et doit différer de `[daemon.ack] api_key` et de `[daemon.incidents] api_key`, puisqu'une clé de lecture égale à une clé d'écriture est cette clé. Donnez-la à Grafana et au Hub pour que rien de ce qui ne fait que lire ne détienne une clé capable d'acquitter ou de fabriquer un incident. Un Hub qui relaie les acks détient `[daemon.ack] api_key` comme second secret, distinct, envoyé sur ces seules écritures. 12 caractères minimum. `PERF_SENTINEL_READ_API_KEY` prime sur cette valeur                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `require_read_key`        | booléen  | `false`                     | Met chaque `GET` (et `HEAD`) de l'API de requête derrière une clé : `/api/findings`, `/api/findings/{trace_id}`, `/api/explain/{trace_id}`, `/api/correlations`, `/api/export/report`, `/api/acks`, `/api/incidents`, `/api/status`, `/api/config` et `/api/energy` répondent `401` sans elle. Chaque route prend `read_api_key` ou la clé d'écriture que son propre handler accepte déjà : `[daemon.ack] api_key` partout sauf sur `GET /api/incidents`, qui prend `[daemon.incidents] api_key` à la place, en `X-API-Key` ou en `Authorization: Bearer`. Les écritures gardent leurs propres clés, et `/metrics`, `/health`, l'ingestion OTLP et la socket JSON restent hors de la porte. Désactivé (défaut), l'API se comporte comme avant et `read_api_key` n'ajoute jamais de porte. `true` sans `read_api_key` (ni `PERF_SENTINEL_READ_API_KEY`) est une erreur de configuration, tout comme `true` avec `[daemon.cors] allowed_origins = ["*"]`. Chaque lecteur a alors besoin de la clé : le Hub (`AuthHeaderName=X-API-Key`), les datasources Grafana, `perf-sentinel query --api-key-file`, `curl …/api/export/report | perf-sentinel report`, le rapport HTML live et le plugin JetBrains. **Ne l'activez pas sur un daemon que lisent les IDE des développeurs tant que le plugin JetBrains installé n'envoie pas de secret par endpoint** : les versions antérieures du plugin n'en envoient aucun et reçoivent `401` à chaque rafraîchissement. Voir [QUERY-API-FR.md](QUERY-API-FR.md#porte-de-lecture) |
| `max_retained_findings`   | entier   | `10000`                     | Nombre maximum de findings récents conservés dans le buffer circulaire du daemon pour l'API de requêtage. Les findings les plus anciens sont évincés quand la limite est atteinte. Plage : 0 à 10 000 000, où `0` désactive complètement le store et libère sa mémoire (recommandé quand `api_enabled = false`)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `max_export_findings`     | entier   | `1000`                      | Nombre maximum de findings portés par un snapshot `/api/export/report`. Distinct du plafond de `/api/findings`, qui pagine une API de consultation, là où celui-ci dimensionne un export : un store contenant des dizaines de milliers de findings n'en expédie qu'une tranche, la plus récente, et le snapshot l'annonce dans `warning_details`. L'augmenter alourdit le corps de la réponse, et le HTML qui en est rendu, de quelques Ko par finding. Au-delà d'environ 2000 le snapshot dépasse la limite de corps de 8 Mio avec laquelle `query inspect` et `query monitor` le récupèrent, et le daemon émet un avertissement. Plage : 0 à 100 000, où `0` n'exporte que l'enveloppe (chiffres green, aucun finding). Comme la `quality_gate` exportée compte les findings de cette tranche, à `0` ses trois règles de comptage passent quoi qu'ait détecté le daemon. La quatrième, `io_waste_ratio_max`, lit `green_summary`, qu'aucun plafond ne vide : le verdict ne passe donc pas systématiquement. Il cesse seulement de refléter les findings. Cela suffit à réserver `0` à une sonde de liveness, pas à une sonde d'alerte. Surchargeable au lancement avec `watch --max-export-findings`                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `max_retained_traces`     | entier   | `50`                        | Nombre maximum de traces dont les spans masqués sont conservés pour que `/api/export/report` porte un arbre de spans que le dashboard HTML sait dessiner. Sans cela, la fenêtre de corrélation lâche les spans d'une trace quelques secondes après sa fin et un rapport exporté a des findings sans rien à montrer autour. Coûte de la mémoire proportionnellement à `max_events_per_trace`, d'où un plafond bien plus petit que `max_retained_findings`. Plage : 0 à 10 000, où `0` n'en conserve aucune. Ignoré (traité comme `0`) quand `api_enabled = false` ou `max_retained_findings = 0` : rien ne pourrait servir les arbres                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `ingest_queue_capacity`   | entier   | `1024`                      | Capacité du canal d'ingestion : lots d'événements de span tamponnés entre les listeners et la boucle d'événements. Une fois plein, l'ingestion applique une contre-pression aux producteurs. Augmentez-la pour absorber un trafic plus en rafales, au prix de la mémoire. Plage : 1 à 1 048 576                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `analysis_queue_capacity` | entier   | `1024`                      | Capacité de la file du worker d'analyse : lots évincés et expirés en attente de detect+score. Une fois pleine, des lots entiers sont délestés et comptés sur `perf_sentinel_analysis_shed_batches_total`. Augmentez-la pour tolérer des rafales d'analyse plus longues avant délestage. Plage : 1 à 1 048 576                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `per_service_labels`      | booléen  | `true`                      | Si `perf_sentinel_findings_total` et `perf_sentinel_slow_duration_seconds` portent un label `service` (depuis 0.18.0). La cardinalité est plafonnée par run du daemon (128 services sur les findings, 64 sur l'histogramme), les services au-delà d'un plafond se replient dans `service="_other"` pour que les totaux restent exacts. `false` rend le label vide sur chaque série, restaurant la forme d'avant 0.18, et depuis 0.19.0 la série sans label de l'histogramme n'est pré-chauffée au démarrage que si `per_grouping_labels` est aussi désactivé. Un label vide n'est pas un label : un scrape honor labels (le ServiceMonitor du chart) le supprime et un scrape ordinaire l'écrase avec le nom de la cible. Avec le réglage désactivé, le filtre `Service` du tableau de bord livré liste toujours les services (depuis les compteurs I/O par service), mais en choisir un affiche 0 finding et aucune latence de span lent. Gardez `All` ou laissez le réglage actif. Sans effet sur les compteurs I/O par service (`service_io_ops_total`, `service_avoidable_io_ops_total`, `service_analyzed_io_ops_total`), par service par construction                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `per_grouping_labels`     | booléen  | `true`                      | Si `perf_sentinel_findings_total`, `perf_sentinel_slow_duration_seconds`, `perf_sentinel_service_io_ops_total`, `perf_sentinel_service_avoidable_io_ops_total` et `perf_sentinel_service_analyzed_io_ops_total` portent un label `grouping` à côté de `service` (depuis 0.19.0). Le label contient la valeur du premier attribut présent parmi `[detection] grouping_attributes` (vide quand le span n'en portait aucun), sans sa clé d'attribut : ne configurez donc qu'une clé quand deux attributs peuvent partager une valeur. La cardinalité est plafonnée par run du daemon sur les paires (service, grouping) admises, après les plafonds de services (512 côté analyse, 256 sur l'histogramme, 4096 à l'ingestion). Une paire au-delà garde son service et replie son regroupement dans `grouping="_other"`, pour que `sum by (service)` égale toujours la série de la 0.18.0. Les plafonds comptent des paires, donc ce qui se replie suit le produit services × regroupements (11 services dans 20 namespaces font 220 paires, 100 dans 10 en font 1000). `false` rend le label vide sur chaque série, restaurant la forme de la 0.18.0. Un label vide n'est pas un label, et comme rien n'attache de label de cible `grouping`, la série n'en a aucun : le filtre `Grouping` du tableau de bord livré ne l'atteint que sous `All`. Contrairement à `per_service_labels`, ce réglage gouverne aussi les trois compteurs I/O par service. Il ne s'appelle pas `namespace` parce que Prometheus Operator attache un label de cible `namespace` et que le `honorLabels: true` du chart ferait gagner celui du daemon |
| `findings_stdout`         | booléen  | `true`                      | Si chaque finding est aussi écrit sur stdout, une ligne NDJSON par finding, pour un backend de logs qui suit le conteneur (voir [docs/FR/RUNBOOK-FR.md](RUNBOOK-FR.md)). `false` garde les findings dans l'API de requête, l'archive et le flux `[daemon.hub_export]` seulement, et laisse le log du conteneur aux messages du daemon sur stderr : le réglage d'un daemon dont les findings arrivent déjà dans PerfSentinelHub, où la copie sur stdout ne coûte que du CPU et du disque sur le nœud. Les métriques, le buffer circulaire et chaque export en aval ne changent pas |
| `memory_high_water_pct`   | entier   | `0`                         | Contrôle d'admission par pression mémoire, en pourcentage de la limite mémoire cgroup v2. Quand le ratio de working set (`memory.current` moins le page cache récupérable `inactive_file`, sur `memory.max`) franchit ce seuil, l'ingestion est rejetée avec un statut réessayable (compté sur `perf_sentinel_otlp_rejected_total{reason="memory_pressure"}`, état sur la jauge `perf_sentinel_ingest_memory_pressure`) et reprend une fois l'usage retombé 5 points de pourcentage sous le seuil (hystérésis, pour que l'ingestion n'oscille pas autour de la frontière), bornant la RSS indépendamment de la profondeur de file. `0` désactive le garde-fou (défaut). Linux/cgroup-v2 uniquement, inerte ailleurs, et laisse passer l'ingestion (fail-open) si le cgroup devient illisible. Réglez 80-85 pour garder de la marge au-dessus d'une empreinte typique en régime permanent. Le garde-fou échantillonne à cadence fixe, donc dimensionnez le seuil pour que la marge `limit - mark` dépasse le pic de l'empreinte en vol (un afflux soutenu peut dépasser une marge trop fine, voir [RUNBOOK-FR](RUNBOOK-FR.md#pression-mémoire-ou-oom-du-daemon)). Plage : 0, ou 6 à 100 (1-5 mettrait la borne basse d'hystérésis à zéro ou en dessous)                                                                                                                                                                                                                                                                                                                                                                      |

##### Zones de confort et avertissements au démarrage

Les limites du daemon acceptent toute valeur à l'intérieur de leurs bornes dures (rejetées au chargement de la config), mais `perf-sentinel watch` émet un log `WARN` unique au démarrage quand une valeur sort de la zone de confort recommandée. L'avertissement est informatif : le daemon continue de tourner. Il sert à repérer une valeur inhabituelle définie par erreur.

| Champ                   | Zone de confort        | Pourquoi une valeur hors zone est inhabituelle                                                                                                                    |
|-------------------------|------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `max_payload_size`      | 256 Kio à 16 Mio       | Plus petit risque de rejeter des batches OTLP légitimes. Plus grand augmente la latence d'ingestion et le RSS                                                     |
| `max_active_traces`     | 1 000 à 100 000        | Plus petit déclenche une éviction LRU agressive. Plus grand fait croître la mémoire à peu près linéairement                                                       |
| `max_events_per_trace`  | 100 à 10 000           | Plus petit tronque les traces complexes. Plus grand n'améliore que rarement la qualité de détection                                                               |
| `max_retained_findings` | 100 à 100 000 (ou `0`) | Plus petit évince les findings avant que `/api/findings` ne puisse les servir. Plus grand garde un backlog en mémoire. `0` désactive le store et reste silencieux |
| `trace_ttl_ms`          | 1 000 à 600 000        | Sous 1s, les traces sont vidées avant que les spans lents n'arrivent. Au-dessus de 10min, des traces presque mortes restent en mémoire                            |
| `max_fanout`            | 5 à 1 000              | Plus petit inonde le store de findings de bruit. Plus grand supprime la plupart des détections de fanout                                                          |

Les zones de confort jugent la valeur statique au démarrage. Au
runtime, le daemon les complète avec un conseiller de réglages : quand
les compteurs cumulés depuis le démarrage montrent un réglage
sous-dimensionné pour la charge observée (délestages de file, rejets
d'ingestion, fenêtre de traces presque pleine...),
`/api/export/report` émet des entrées `tuning` dans
`Report.warning_details` nommant le réglage, sa valeur actuelle et
l'ajustement suggéré. Voir [METRICS-FR.md](METRICS-FR.md) section
"Kinds de warning : transitoire vs collant" pour la table des règles.

#### `[daemon.correlation]` (optionnel)

Corrélation temporelle cross-trace en mode daemon. Quand activé, le daemon détecte les co-occurrences récurrentes entre findings de services ou traces différents (ex. "chaque fois que le N+1 dans order-svc se déclenche, une saturation du pool apparaît dans payment-svc dans les 2 secondes").

| Champ                | Type     | Défaut  | Description                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
|----------------------|----------|---------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `enabled`            | booléen  | `false` | Active la corrélation cross-trace. Nécessite le mode daemon `watch` avec un trafic soutenu pour des résultats utiles                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `window_minutes`     | entier   | `10`    | Fenêtre glissante en minutes sur laquelle les co-occurrences et les occurrences des sources sont comptées. Plage 1 à 10080 (7 jours), vérifiée quand `enabled = true`. La mémoire ne croît pas avec les occurrences de la fenêtre : l'appariement ne garde que les findings analysés dans les derniers `lag_threshold_ms + 2 x trace_ttl_ms`, donc le deque d'appariement et le parcours que chaque finding en fait croissent avec `trace_ttl_ms` (environ une minute de findings avec le TTL par défaut de 30 s), et la fenêtre ne fixe que la période couverte par les compteurs par paire et par endpoint. Chaque endpoint distinct (template compris) vu dans la fenêtre est conservé, donc la mémoire croît avec le nombre d'endpoints distincts de la fenêtre                  |
| `lag_threshold_ms`   | entier   | `5000`  | Décalage temporel maximum en millisecondes entre deux findings pour les considérer co-occurrents, mesuré entre les horodatages de premier span des findings eux-mêmes (`first_timestamp`), pas entre les instants où le daemon les a analysés. Le finding à l'horodatage le plus ancien est la source                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `min_co_occurrences` | entier   | `5`     | Nombre minimum de co-occurrences sur la fenêtre avant de remonter une corrélation. Chaque occurrence source compte au plus une fois par paire                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `min_confidence`     | flottant | `0.7`   | Score de confiance minimum (0.0 à 1.0) pour remonter une corrélation. Calculé comme `co_occurrence_count / source_total_occurrences`, tous deux comptés sur la même fenêtre                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `max_tracked_pairs`  | entier   | `10000` | Nombre maximum de paires de findings retenues simultanément. Le plafond borne ce que le corrélateur garde, pas ce qu'un batch parcourt : une topologie large balaie le produit croisé des findings entrants et de l'horizon d'appariement quelle que soit cette valeur, donc l'abaisser fait refuser davantage plutôt qu'allouer moins. Les paires croissent comme le nombre de types de findings multiplié par le nombre de services : une poignée de services peut dépasser le défaut, et au-delà du plafond le corrélateur évince les paires au plus faible compte et les plus anciennes, à égalité dans l'ordre des paires, donc `/api/correlations` renvoie un sous-ensemble sans que la sortie le signale. `perf_sentinel_correlator_pairs_evicted_total` est le signal, et le daemon journalise un avertissement à la première éviction. Pas de contrôle de zone de confort au démarrage |

```toml
[daemon.correlation]
enabled = true
window_minutes = 10
lag_threshold_ms = 5000
min_co_occurrences = 5
min_confidence = 0.7
```

Une fenêtre de 24 h convient aux motifs qui se répètent sur la journée. Elle coûte la même mémoire d'appariement que le défaut. Seuls les compteurs couvrent une période plus longue :

```toml
[daemon.correlation]
enabled = true
window_minutes = 1440
lag_threshold_ms = 2000
min_co_occurrences = 10
min_confidence = 0.7
max_tracked_pairs = 50000
```

Les corrélations sont exposées via `GET /api/correlations` (quand `api_enabled = true`) et figées sous `correlations` dans `GET /api/export/report`. Le flux stdout du daemon ne les porte jamais.

Avec `[daemon.hub_export]` actif aussi, le daemon pousse les périodes actives de chaque paire
vers PerfSentinelHub 0.6.0 ou ultérieur sous forme d'épisodes, depuis 0.30.0, donc une
corrélation y survit à la fenêtre glissante. Un épisode se ferme une fois que la dernière
co-occurrence de la paire est restée immobile pendant `min(window_minutes, 30 minutes)`.
`[daemon.hub_export]` plus bas décrit ce qui part.

#### `[daemon.ack]` (optionnel, depuis 0.5.20)

Store d'acks runtime côté daemon. Complète les acks TOML CI (voir
`ACKNOWLEDGMENTS-FR.md`) avec un fichier JSONL append-only muté via les
endpoints HTTP `POST` / `DELETE` `/api/findings/{signature}/ack`.

| Champ          | Type    | Défaut                                                  | Description                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
|----------------|---------|---------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `enabled`      | booléen | `true`                                                  | Active les endpoints ack du daemon. Quand `false`, `POST` / `DELETE` / `GET /api/acks` retournent 503 Service Unavailable, et `GET /api/findings` saute le filtre ack                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| `storage_path` | chaîne  | `<data_local_dir>/perf-sentinel/acks.jsonl`             | Surcharge de l'emplacement du fichier JSONL. Résolu au runtime via `dirs::data_local_dir()` (XDG sur Linux, Library/Application Support sur macOS) en l'absence de surcharge. La politique d'erreur dépend de la source : une surcharge explicite qui échoue à s'ouvrir est fatale au démarrage, un chemin par défaut qui ne peut pas être résolu ou ouvert ne produit qu'un WARN et laisse les deux routes d'écriture d'ack en 503 (`GET /api/acks` n'est gardé que par l'authentification et répond toujours 200 avec une liste vide). Pas de repli sur `/tmp` car le fichier contient des données d'audit qui doivent survivre à un redémarrage. Les conteneurs minimaux sans `HOME` (dont l'image publiée `FROM scratch`) tombent dans le second cas, définissez donc ce champ explicitement là-bas |
| `api_key`      | chaîne  | *(absent)*                                              | Secret optionnel gardant l'accès aux acks. Quand défini, `POST` et `DELETE` sur `/api/findings/{signature}/ack` **et** `GET /api/acks` exigent que l'en-tête `X-API-Key`, ou un `Authorization: Bearer` portant la même clé, corresponde (comparaison en temps constant via `subtle`, ce `GET` acceptant aussi `[daemon] read_api_key`). `GET /api/findings` reste non authentifié sauf si `[daemon] require_read_key = true`. Définissez la variable d'environnement `PERF_SENTINEL_ACK_API_KEY` pour surcharger cette valeur et garder la clé hors de la config versionnée. La variable d'env a la priorité quand elle est présente, même convention que `PERF_SENTINEL_EMAPS_TOKEN`. Une chaîne vide (ou une variable d'env définie à vide) est rejetée au chargement de la config                                                              |
| `toml_path`    | chaîne  | `".perf-sentinel-acknowledgments.toml"` (relatif à CWD) | Surcharge du fichier TOML d'acks CI. Lu au démarrage, puis relu chaque minute pour qu'une modification s'applique sans redémarrage, ce qui compte quand le fichier est une ConfigMap montée. Une relecture en échec conserve les acks précédents et journalise un avertissement, tout comme un fichier disparu : ni un fichier à moitié écrit ni un volume démonté ne désacquitte quoi que ce soit. Seul un chemin configuré explicitement est relu, jamais le défaut relatif à CWD, et `enabled = false` arrête la relecture avec le reste. À régler en chemin absolu pour les déploiements systemd ou en conteneur où CWD n'est pas la racine du repo                                                                                                                                                 |

```toml
[daemon.ack]
enabled = true
storage_path = "/var/lib/perf-sentinel/acks.jsonl"
# api_key = "<rotate-this>"
toml_path = "/etc/perf-sentinel/acknowledgments.toml"
```

Le fichier JSONL est rejoué et atomiquement réécrit (via tmp + rename)
à chaque redémarrage du daemon, donc des cycles `ack` / `unack`
répétés ne peuvent pas s'accumuler au-delà de leur état actif net. Sur
Unix, le fichier est créé avec le mode `0600` (lecture-écriture
propriétaire uniquement).

#### `[daemon.hub_export]` (optionnel)

Export asynchrone et borné des findings live vers PerfSentinelHub. Le daemon
fusionne les findings répétés par signature et ne conserve que leur dernière
valeur : un problème très fréquent ne déclenche donc pas une requête réseau
par détection.

| Champ                  | Type    | Défaut     | Description                                                                                                   |
|------------------------|---------|------------|---------------------------------------------------------------------------------------------------------------|
| `enabled`              | booléen | `false`    | Active l'export Hub. La détection reste non bloquante si le Hub est indisponible                              |
| `endpoint`             | chaîne  | *(absent)* | URL du Hub terminant par `/api/import/findings`                                                               |
| `source_id`            | chaîne  | *(absent)* | Identifiant de source Hub : 1 à 64 lettres ASCII, chiffres, `.`, `_` ou `-`                                   |
| `api_key_file`         | chaîne  | *(absent)* | Fichier contenant la clé d'import de la source. Requis si activé. La clé doit contenir au moins 32 caractères |
| `batch_size`           | entier  | `100`      | Findings par requête, de 1 à 100                                                                              |
| `flush_interval_secs`  | entier  | `5`        | Délai normal maximal de regroupement, de 1 à 300 secondes                                                     |
| `request_timeout_secs` | entier  | `10`       | Durée maximale d'un envoi avant que le daemon abandonne et réessaie, de 1 à 60 secondes                       |
| `max_pending`          | entier  | `10000`    | Nombre maximal de signatures distinctes en attente, de 1 à 1 000 000                                          |

```toml
[daemon.hub_export]
enabled = true
endpoint = "https://hub.example.com/api/import/findings"
source_id = "production-a"
api_key_file = "/run/secrets/perf-sentinel-hub-api-key"
batch_size = 100
flush_interval_secs = 5
request_timeout_secs = 10
max_pending = 10000
```

`request_timeout_secs` s'applique aux envois de findings, d'incidents et de corrélations.
Augmentez-le pour un Hub qui répond lentement pendant un rattrapage, plutôt que de laisser
chaque envoi expirer, reculer jusqu'à cinq minutes et remplir la table en attente. Un arrêt
propre laisse toujours 10 secondes à la vidange, donc un délai plus long peut y couper un
envoi. Une seule tâche envoie les trois sortes à tour de rôle : tant qu'un envoi attend le
Hub, les incidents et les lectures de corrélations attendent avec lui. Donnez au Hub le
temps qu'il lui faut, pas davantage.

La structure en attente est une table de dernières valeurs, pas une file
illimitée. Une signature est envoyée dès sa première découverte ou si sa
sévérité empire, puis rafraîchie au plus une fois par heure tant qu'elle se
répète. La table en attente et le cache des succès récents sont chacun bornés
à `max_pending`. Évincer du cache des succès récents n'est pas une perte et
n'est pas compté, alors qu'une éviction de la table en attente, un finding trop
gros et un lot rejeté par le Hub avec un 4xx non rejouable augmentent tous
`perf_sentinel_hub_export_dropped_total`. Un échec HTTP conserve le lot
fusionné et déclenche une nouvelle tentative avec backoff exponentiel et
jitter. Un redémarrage efface les deux caches. Requêtes et corps JSON sont
bornés à 100 findings et 2 Mio. Utilisez HTTPS hors réseau privé de confiance.

Avec `[daemon.incidents]` actif aussi, le daemon pousse ses incidents vers
`/api/import/incidents` à côté d'`endpoint`, avec la même clé et le même identifiant de
source, depuis 0.29.0. Un incident part quand il est enregistré, quand son alerte se
résout et quand la passe de consolidation lui ajoute des findings gelés, au plus 10 par
requête et 4 Mio par corps. Sa table suit celle des findings : le dernier état par
identifiant d'incident, retentée avec le même backoff sur sa propre horloge, vidée à
l'arrêt. Elle garde au plus `max_pending` ou 1 000 incidents, le plus petit des deux,
puisqu'un incident peut porter mille findings gelés. Un incident de plus de 4 Mio à lui seul est abandonné, et le poll horaire du Hub
peut encore le copier. Un Hub qui répond 404 ou 405 sur cette route la précède : le daemon
journalise un avertissement et cesse de pousser les incidents jusqu'à son redémarrage,
tandis que les findings continuent. Un Hub plus ancien avec la connexion active répond 401
à la place, donc le daemon retente et journalise chaque tentative : mettez le Hub à jour
d'abord. `perf_sentinel_hub_export_incidents_pending` et
`perf_sentinel_hub_export_incidents_dropped_total` comptent ce qui attend et ce qui est
abandonné.

Avec `[daemon.correlation]` actif aussi, le daemon pousse ses corrélations cross-trace vers
`/api/import/correlations` à côté d'`endpoint`, avec la même clé et le même identifiant de
source, depuis 0.30.0, sous forme d'épisodes : un par période pendant laquelle une paire a
continué de se produire ensemble. Un épisode s'ouvre quand la dernière co-occurrence d'une
paire (`last_seen`) est récente, la suit tant qu'elle avance, et se ferme une fois qu'elle est
restée immobile le temps de l'écart d'épisode, `min(window_minutes, 30 minutes)`, 10 minutes
par défaut. Une copie part quand un épisode s'ouvre, quand sa confiance ou son nombre de
co-occurrences maximal monte, au plus une fois par minute tant qu'il reste actif, et quand il
se ferme, au plus 100 épisodes par requête et 1 Mio par corps. Un arrêt propre ferme chaque
épisode ouvert avant que l'exporteur se vide. La table garde la dernière copie par épisode,
bornée par `max_pending`, et retente avec le même backoff sur sa propre horloge. Un épisode
de plus de 1 Mio à lui seul est abandonné. Un Hub qui répond 404 ou 405 sur cette route la
précède : le daemon journalise un avertissement et cesse de pousser les corrélations jusqu'à
son redémarrage, tandis que les findings et les incidents continuent. Un Hub plus ancien avec
la connexion active répond 401 à la place, donc le daemon retente et journalise chaque
tentative : mettez d'abord le Hub en PerfSentinelHub 0.6.0.
`perf_sentinel_hub_export_correlations_pending` et
`perf_sentinel_hub_export_correlations_dropped_total` comptent ce qui attend et ce qui est
abandonné.

À l'arrêt propre (SIGTERM, `helm upgrade`, redémarrage progressif), l'exporteur
vide ce qu'il détient encore avant que le processus ne se termine, pendant au
plus 10 secondes. Ce budget empêche un Hub injoignable de retenir le daemon
au-delà du délai de grâce de l'orchestrateur, où le signal suivant est SIGKILL
et où plus rien n'est transmis. Quand le budget expire, les findings encore en
attente sont perdus et un `WARN` en donne le nombre. Dimensionnez
`terminationGracePeriodSeconds` au-dessus de ce budget pour que le drain ait la
place de s'exécuter, sinon le pod est tué en plein envoi.

Montez la clé API depuis un fichier adossé à un Secret. Ne la mettez ni dans
une ConfigMap ni dans `.perf-sentinel.toml`. Avec le chart Helm, utilisez
`extraVolumes` et `extraVolumeMounts` pour exposer la clé au chemin
`api_key_file` configuré.

#### `[daemon.incidents]` (optionnel, depuis 0.20.0)

Webhooks d'incident entrants, pour qu'une alerte donne au daemon le
moment où un service est tombé ou a saturé, et que le daemon fige les
findings de la fenêtre avant que le ring ne les évince. Voir
[QUERY-API-FR.md](QUERY-API-FR.md) pour les endpoints. Daemon uniquement,
désactivé par défaut, et à ne pas confondre avec
`[daemon] memory_high_water_pct`, qui désigne le cgroup du daemon
lui-même.

| Champ             | Type    | Défaut               | Description                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
|-------------------|---------|----------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `enabled`         | booléen | `false`              | Expose `POST` et `GET /api/incidents`. Une surface d'écriture entrante est opt-in. Les deux routes répondent 503 sans elle                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `api_key`         | chaîne  | *(absent)*           | **Obligatoire quand activé**, sinon erreur de configuration, parce que la route écrit. Comparé en temps constant à `X-API-Key` ou `Authorization: Bearer`, l'un ou l'autre, 12 caractères minimum, et le `GET` accepte aussi `[daemon] read_api_key`. `PERF_SENTINEL_INCIDENTS_API_KEY` prime sur cette valeur                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| `lookback_ms`     | entier  | `300000`             | Profondeur de findings qu'un incident posté fige, de 1000 à 86400000                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `max_retained`    | entier  | `200`                | Incidents gardés dans le ring en mémoire, de 1 à 1000, chacun portant jusqu'à 1000 findings figés. Avec `archive_path` posé, le ring est rechargé depuis l'archive au démarrage, jusqu'à ce nombre d'incidents, il survit donc aux redémarrages                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `service_label`   | chaîne  | `service`            | Libellé d'alerte portant le nom de service Perf Sentinel. Une alerte qui ne le porte pas est refusée, c'est la clé de jointure avec les findings                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `kind_label`      | chaîne  | `perf_sentinel_kind` | Libellé d'alerte portant le genre : `oom_kill`, `memory_saturation`, `restart`, `deploy` ou `other`. Tout le reste vaut `other`, jamais deviné depuis `alertname`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `namespace_label` | chaîne  | `namespace`          | Libellé d'alerte portant le namespace, celui que les alertes kube-prometheus portent déjà. Optionnel, jamais un motif de refus : sa valeur est portée sur l'incident comme `namespace`, le paramètre `namespace` de `GET /api/incidents` filtre dessus, et elle restreint le gel à ce namespace en écartant les findings dont l'attribut de regroupement `k8s.namespace.name` en nomme un autre. Il faut pour cela `k8s.namespace.name` parmi `[detection] grouping_attributes`, et une alerte sans ce libellé fige par service seul                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `archive_path`    | chaîne  | *(absent)*           | Ajoute chaque nouvel incident, fermeture et consolidation à ce fichier JSON par lignes, ouvert au démarrage pour qu'un mauvais chemin fasse échouer le daemon. Absent signifie que le ring en mémoire est le seul enregistrement, et un événement mémoire au niveau du nœud qui tue le service observé emporte souvent un daemon colocalisé. Append-only, le dernier enregistrement d'un id fait foi, créé en `0600` et ramené à ce mode quand un montage de volume l'affaiblit, refusé quand le fichier n'appartient pas au daemon, sans rotation. Relu au démarrage, avant que l'API ne serve, sur ses 256 Mio les plus récents au plus : le dernier enregistrement de chaque id, les `max_retained` derniers arrivés, dans l'ordre où le ring les tenait, les lignes illisibles ignorées avec un seul avertissement, rien n'est réécrit. Une ligne ne dit pas si elle enregistre ou met à jour, donc une fermeture ou une consolidation dont l'enregistrement précède cette lecture, a été perdu ou a été écarté par un `max_retained` plus petit compte comme une arrivée à sa place dans le fichier |

```toml
[daemon.incidents]
enabled = true
# api_key = "<rotate-this>"      # ou PERF_SENTINEL_INCIDENTS_API_KEY
lookback_ms = 300000
```

La fenêtre qu'un incident posté fige est
`[at_ms - lookback_ms, at_ms + 2 * trace_ttl_ms]`, résolue avec les deux
bornes, donc `seen_count` et `first_seen_ms` des findings figés décrivent
la fenêtre plutôt que l'historique retenu entier. Voir
[QUERY-API-FR.md](QUERY-API-FR.md) pour la raison de cette fermeture
après l'incident et pour la passe de consolidation qui en remplit la
queue.

#### `[daemon.cors]` (optionnel, depuis 0.5.23)

Cross-origin resource sharing pour les endpoints de requêtage `/api/*`
du daemon. Désactivé par défaut (aucun en-tête
`Access-Control-Allow-Origin` n'est émis, la posture loopback-only est
préservée). À activer quand un client navigateur doit appeler le
daemon, typiquement le rapport HTML en mode live
(`perf-sentinel report --daemon-url <URL>`, voir `HTML-REPORT-FR.md`).

**Portée** : la couche CORS est branchée uniquement sur le sous-router
`/api/*`. Le chemin d'ingestion OTLP (`/v1/traces`), l'exposition
Prometheus (`/metrics`) et le liveness probe (`/health`) ne sont PAS
exposés en cross-origin, même en mode wildcard. Les pages navigateur
ne peuvent pas envoyer de traces, scraper `/metrics` ou appeler
`/health` quel que soit `allowed_origins`, puisque les clients
navigateur n'ont aucun usage légitime pour ces surfaces.

**Exposition des endpoints de lecture** : chaque GET `/api/*`
(`/api/findings`, `/api/acks`, `/api/status`, `/api/correlations`,
`/api/explain/*`, `/api/export/report`) est non authentifié sauf si
`[daemon] require_read_key = true`, en cohérence avec la posture
loopback-only pré-0.5.23. Une fois qu'une
origine est autorisée, tout onglet de navigateur sur cette origine peut
lire chaque signature de finding, métadonnée d'ack et export de trace
que le daemon retient. **N'autorisez que les origines auxquelles vous
faites confiance pour voir l'ensemble des données du daemon.** Mélanger
des origines non fiables avec le mode wildcard (`["*", "https://x"]`)
est rejeté au chargement de la config.

| Champ             | Type          | Défaut | Description                                                                                                                                                                                                                                                                                                           |
|-------------------|---------------|--------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `allowed_origins` | array<string> | `[]`   | Liste des origines autorisées à appeler la surface `/api/*` du daemon. `["*"]` est le mode wildcard (développement uniquement, sans credentials). Une liste non-wildcard autorise les origines exactes. Chaque entrée hors wildcard doit être une origine complète (scheme + hôte + port optionnel), sans slash final |

Exemple wildcard (développement) :

```toml
[daemon.cors]
allowed_origins = ["*"]
```

Exemple production (liste blanche) :

```toml
[daemon.cors]
allowed_origins = [
    "https://reports.example.com",
    "https://gitlab.example.com",
]
```

Méthodes autorisées : `GET`, `POST`, `DELETE`, `OPTIONS`.
En-têtes autorisés : `Content-Type`, `X-API-Key`. (`X-User-Id` n'est
pas annoncé parce que le daemon ne l'impose pas côté serveur. Le
champ `by` sur le corps d'un POST d'ack est attesté par l'opérateur
uniquement.)
Préflight `Access-Control-Max-Age` : 120 secondes. Assez long pour
amortir l'aller-retour OPTIONS sur une interaction typique, assez
court pour qu'une liste blanche resserrée prenne effet au prochain
préflight navigateur sans rafraîchissement forcé.

La couche CORS ne positionne pas `Access-Control-Allow-Credentials: true`,
incompatible avec `["*"]` et inutile car le daemon authentifie via
l'en-tête `X-API-Key` et non via des cookies. Les navigateurs sur une
origine non autorisée reçoivent une réponse sans en-tête
`Access-Control-Allow-Origin` et la requête est bloquée côté client,
sans rejet côté daemon.

Les origines qui ne se parsent pas comme une valeur d'en-tête HTTP
valide (typiquement un copier-coller avec des caractères de contrôle)
sont écartées au démarrage avec un log `warn!` et le reste de la
liste est honoré. Si toutes les entrées sont invalides, la couche est
désactivée entièrement. Si `[daemon] api_enabled = false`, la couche
CORS est ignorée (le sous-router `/api/*` n'est pas monté de toute
façon) et un `warn!` signale la config inutilisée.

Depuis 0.5.27, combiner
`allowed_origins = ["*"]` avec `[daemon.ack] api_key` émet aussi un
`warn!` au démarrage. Le mode CORS wildcard combiné à une auth
`X-API-Key` autorise n'importe quelle origine navigateur à rejouer
une clé capturée à travers le daemon, même sans cookie ni mode
`Allow-Credentials`. Autorisez des origines explicites pour les
déploiements de production qui configurent la clé API.

### `[reporting]`

Paramètres de divulgation publique consommés par `disclose`, `hash-bake` et `verify-hash`. La section entière est optionnelle. Une section absente signifie que l'opérateur n'a jamais demandé de divulgation périodique. Guide complet dans `docs/FR/REPORTING-FR.md`, référence des champs dans `docs/FR/SCHEMA-FR.md`.

| Champ                   | Type   | Défaut         | Description                                                                                                                                                                                 |
|-------------------------|--------|----------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `intent`                | chaîne | *(non défini)* | `internal`, `official` ou `audited`. Lu au démarrage du daemon uniquement : `audited` le fait refuser de démarrer (pas implémenté), `official` exige `org_config_path` et valide ce fichier |
| `org_config_path`       | chaîne | *(non défini)* | Chemin vers le TOML organisation/scope/méthodologie, exigé quand `intent = "official"`                                                                                                      |
| `confidentiality_level` | chaîne | *(non défini)* | `internal` ou `public`. **Réservé**, validé puis inutilisé : la valeur publiée vient de `disclose --confidentiality`                                                                        |
| `disclose_output_path`  | chaîne | *(non défini)* | **Réservé**, sans effet aujourd'hui, c'est `disclose --output` qui écrit le rapport                                                                                                         |
| `disclose_period`       | chaîne | *(non défini)* | `calendar-quarter`, `calendar-month`, `calendar-year` ou `custom`. **Réservé**, inutilisé, voir `disclose --period-type`                                                                    |

`disclose` ne lit aucune de ces clés : il prend `--intent`, `--confidentiality`, `--period-type`, `--org-config` et `--output` sur la ligne de commande. Cette section conditionne encore le démarrage du daemon via `intent` et `org_config_path`.

La sous-section `[reporting.sigstore]` porte les endpoints Sigstore, Rekor étant le journal de transparence et Fulcio l'autorité de certification. **Les deux sont réservés : ils sont parsés puis inutilisés.** `verify-hash` délègue la vérification de signature au binaire `cosign` et appelle `cosign verify-blob` sans `--rekor-url` ni `--fulcio-url`, cosign suit donc sa propre configuration et renseigner l'une de ces clés ici n'a aucun effet. Pointez une instance Sigstore privée sur cosign lui-même en attendant que ce soit câblé.

| Champ        | Type   | Défaut                        | Description                                              |
|--------------|--------|-------------------------------|----------------------------------------------------------|
| `rekor_url`  | chaîne | `https://rekor.sigstore.dev`  | Endpoint du journal de transparence Rekor. Réservé.      |
| `fulcio_url` | chaîne | `https://fulcio.sigstore.dev` | Endpoint de l'autorité de certification Fulcio. Réservé. |

## Configuration minimale

Un fichier vide ou l'absence de fichier utilise tous les défauts. Une configuration minimale pour la CI peut se limiter aux seuils :

```toml
[thresholds]
n_plus_one_sql_critical_max = 0
io_waste_ratio_max = 0.25
```

## Exemple de configuration complète

```toml
[thresholds]
n_plus_one_sql_critical_max = 0
n_plus_one_http_warning_max = 3
n_plus_one_messaging_warning_max = 3
io_waste_ratio_max = 0.30
# min_usable_span_ratio = 0.9   # absent = désactivé, voir le tableau thresholds

[detection]
n_plus_one_min_occurrences = 5
window_duration_ms = 500
slow_query_threshold_ms = 500
slow_query_min_occurrences = 3
slow_query_window_minutes = 15
max_fanout = 20
chatty_service_min_calls = 15
pool_saturation_concurrent_threshold = 10
serialized_min_sequential = 3
# Heuristique de récupération pour le SQL déjà paramétré : "auto", "strict",
# "always", "never". La barre de variance ci-dessous est ce qui sépare un vrai
# N+1 d'une répétition servie par le cache. La relever sur un runtime instable
# comme PHP-FPM, où les répétitions d'une même requête en cache dépassent 0,5
# et sont lues comme des N+1.
sanitizer_aware_classification = "auto"
sanitizer_aware_min_cv = 0.5

[green]
enabled = true
default_region = "eu-west-3"

[daemon]
listen_address = "127.0.0.1"
listen_port_http = 4318
listen_port_grpc = 4317
json_socket = "/tmp/perf-sentinel.sock"
max_active_traces = 10000
trace_ttl_ms = 30000
sampling_rate = 1.0
max_events_per_trace = 1000
max_payload_size = 16777216
# Optionnel : activer le TLS sur les listeners gRPC et HTTP.
# Les deux champs doivent être renseignés ensemble (ou les deux absents pour TCP en clair).
# tls_cert_path = "/etc/tls/server-cert.pem"
# tls_key_path = "/etc/tls/server-key.pem"
api_enabled = true
max_retained_findings = 10000
max_retained_traces = 50
# Optionnel : régler les files bornées (valeurs par défaut affichées).
# Augmentez sous charge en rafales pour réduire la contre-pression
# d'ingestion / le délestage d'analyse.
ingest_queue_capacity = 1024
analysis_queue_capacity = 1024
# Optionnel : label `service` sur les findings et l'histogramme des spans
# lents (borné par les plafonds de cardinalité du daemon). false restaure
# la forme d'avant 0.18.
per_service_labels = true
# Optionnel : label `grouping` à côté de `service` sur les mêmes séries et
# sur les compteurs I/O par service (plafonné sur les paires (service,
# grouping) admises). false restaure la forme 0.18.
per_grouping_labels = true

# Optionnel : écrire chaque finding sur stdout, une ligne NDJSON (défaut true).
# false quand les findings arrivent déjà dans PerfSentinelHub.
findings_stdout = true

# Optionnel : rejeter l'ingest OTLP quand la mémoire cgroup franchit ce
# pourcentage de la limite du conteneur, bornant la RSS contre l'OOM. 0
# désactive (défaut). Linux/cgroup-v2 uniquement. Réglez 80-85 pour garder
# de la marge au-dessus du régime permanent.
memory_high_water_pct = 0

# Optionnel : corrélation cross-trace (mode daemon uniquement)
# [daemon.correlation]
# enabled = true
# window_minutes = 10
# lag_threshold_ms = 5000
```

## Migration depuis 0.5.x

Huit clés top-level legacy ont été dépréciées en 0.5.26 et retirées en 0.6.0. Une configuration 0.5.x qui en utilise encore une échoue désormais au chargement avec un message de migration explicite, plutôt que de se rabattre silencieusement sur la valeur par défaut. Migrez vers la forme sectionnée ci-dessous avant la mise à jour.

| Retirée (top-level)    | Utiliser à la place          | Section       |
|------------------------|------------------------------|---------------|
| `n_plus_one_threshold` | `n_plus_one_min_occurrences` | `[detection]` |
| `window_duration_ms`   | `window_duration_ms`         | `[detection]` |
| `listen_addr`          | `listen_address`             | `[daemon]`    |
| `listen_port`          | `listen_port_http`           | `[daemon]`    |
| `max_active_traces`    | `max_active_traces`          | `[daemon]`    |
| `trace_ttl_ms`         | `trace_ttl_ms`               | `[daemon]`    |
| `max_events_per_trace` | `max_events_per_trace`       | `[daemon]`    |
| `max_payload_size`     | `max_payload_size`           | `[daemon]`    |

Exemple de migration. Avant (0.5.x) :

```toml
n_plus_one_threshold = 5
listen_port = 4318
max_payload_size = 2097152
```

Après (0.6.0+) :

```toml
[detection]
n_plus_one_min_occurrences = 5

[daemon]
listen_port_http = 4318
max_payload_size = 2097152
```

Le chargement d'un fichier 0.5.x sur 0.6.0 retourne une `ConfigError::Validation` dont le message nomme à la fois la clé retirée et son remplacement, donc un simple tail du flux d'erreur indique exactement quoi modifier.

## Variables d'environnement

Les fichiers de configuration ne doivent jamais contenir de secrets. Pour les valeurs sensibles (clés API, tokens), utilisez des variables d'environnement dans vos outils de déploiement. Perf Sentinel en lit une liste fixe, chacune surchargeant le champ de config correspondant quand elle est définie et nettoyée d'un saut de ligne final, donc un Secret Kubernetes peut l'alimenter directement :

| Variable                          | Surcharge                                                       | Lue par                                 |
|-----------------------------------|-----------------------------------------------------------------|-----------------------------------------|
| `PERF_SENTINEL_EMAPS_TOKEN`       | l'`api_key` Electricity Maps                                    | daemon et batch                         |
| `PERF_SENTINEL_FINGRID_API_KEY`   | `[green.fingrid] api_key`                                       | daemon et batch                         |
| `PERF_SENTINEL_REE_ESIOS_API_KEY` | `[green.ree_esios] api_key`                                     | daemon et batch                         |
| `PERF_SENTINEL_OPEN_ELECTRICITY_API_KEY` | `[green.open_electricity] api_key`                       | daemon et batch                         |
| `PERF_SENTINEL_ACK_API_KEY`       | `[daemon.ack] api_key`                                          | daemon et batch                         |
| `PERF_SENTINEL_INCIDENTS_API_KEY` | `[daemon.incidents] api_key`                                    | daemon et batch                         |
| `PERF_SENTINEL_READ_API_KEY`      | `[daemon] read_api_key`                                         | daemon et batch                         |
| `PERF_SENTINEL_DAEMON_API_KEY`    | le `--api-key-file` de `ack` et de chaque sous-commande `query` | CLI, envoyée en `X-API-Key`             |
| `PERF_SENTINEL_DAEMON_URL`        | l'URL `--daemon` des commandes `ack` et `query`                 | CLI                                     |
| `PERF_SENTINEL_HUB_TOKEN`         | le jeton enregistré par `login`, pour `hub`                     | CLI, envoyée en `Authorization: Bearer` |
| `PERF_SENTINEL_HUB_URL`           | l'URL `--hub` de `login` et `hub`                               | CLI                                     |

Les surcharges s'appliquent à chaque exécution, commandes batch et exécution sans aucun fichier de config comprises, donc un job qui hérite du Secret du daemon doit lui aussi porter des valeurs valides (la clé d'incidents ne compte qu'une fois `[daemon.incidents] enabled` posé). Une variable définie à la chaîne vide compte comme définie : une clé vide est rejetée au chargement de la config plutôt qu'ignorée en silence, donc un Secret monté vide fait échouer toute commande qui la charge, daemon compris, au lieu d'ouvrir la route. Les clés de sources de données font exception : un `PERF_SENTINEL_EMAPS_TOKEN`, `PERF_SENTINEL_FINGRID_API_KEY`, `PERF_SENTINEL_REE_ESIOS_API_KEY` ou `PERF_SENTINEL_OPEN_ELECTRICITY_API_KEY` vide ou blanc compte comme absent, puisqu'une clé manquante ne fait que désactiver sa source (avec un avertissement pour Fingrid et e·sios, Open Electricity appelant alors en anonyme) et n'ouvre rien.

### Proxy sortant et CA privée

Quatre autres variables régissent chaque appel HTTPS sortant, depuis le daemon (sources d'énergie, Electricity Maps, export vers le Hub) comme depuis la CLI (`tempo`, `jaeger-query`, `pg-stat` et `mysql-stat` avec `--prometheus`, `report` avec une source Prometheus, `query`, `ack`, `login`, `logout`, `hub`, `verify-hash --url`) :

| Variable                     | Effet                                                                                                                                                                                                             |
|------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `HTTPS_PROXY`, `https_proxy` | Proxy des destinations `https://`, joint par un tunnel HTTP `CONNECT`. Les identifiants de son URL (`http://user:pass@proxy:3128`) deviennent une authentification Basic auprès du proxy                          |
| `ALL_PROXY`, `all_proxy`     | Proxy utilisé quand `HTTPS_PROXY` n'est pas posée, est vide ou n'est pas une URL exploitable                                                                                                                      |
| `NO_PROXY`, `no_proxy`       | Destinations jointes directement : noms d'hôte, domaines avec leurs sous-domaines, adresses IP, plages CIDR, ou `*` (`localhost,.svc,10.0.0.0/8`). La boucle locale n'est jointe directement que si elle y figure |
| `SSL_CERT_FILE`              | Bundle PEM dont les certificats s'ajoutent aux racines Mozilla embarquées, en général la CA racine d'un proxy qui inspecte le TLS ou d'une PKI interne                                                            |

Les destinations `http://` sont toujours jointes directement, et `HTTP_PROXY` est ignorée : ces appels visent des services internes, qu'un proxy posé à l'échelle du cluster ne doit pas détourner. Le proxy lui-même est joint en HTTP simple, et le TLS va de bout en bout entre Perf Sentinel et la destination, à l'intérieur du tunnel. Seule une URL de proxy en `http://`, ou sans schéma, est utilisée : une URL de proxy SOCKS ou `https://` produit un seul avertissement par processus, et l'appel part en direct. L'orthographe en majuscules de chaque variable est lue en premier. Les variables de proxy sont ignorées quand `REQUEST_METHOD` est posée, la garde CGI de la bibliothèque sous-jacente. Un `SSL_CERT_FILE` illisible, ou qui ne contient aucun certificat, produit un avertissement et laisse les racines embarquées en place.

Un daemon ou un job de CI qui hérite déjà de `HTTPS_PROXY`, posée à l'échelle de la plateforme ou injectée par un webhook, fait passer ses appels `https://` par ce proxy. Une destination `https://` que le proxy ne peut pas joindre, comme un PerfSentinelHub dans le cluster, un BMC Redfish ou un daemon interrogé en `https://localhost`, doit figurer dans `NO_PROXY`.

Avec le chart Helm, posez-les via `extraEnv` et montez le bundle depuis une ConfigMap ou un Secret :

```yaml
extraEnv:
  - name: HTTPS_PROXY
    value: http://proxy.corp.example:3128
  - name: NO_PROXY
    value: localhost,.svc,.cluster.local,10.0.0.0/8
  - name: SSL_CERT_FILE
    value: /etc/perf-sentinel/ca/corp-root.pem
extraVolumes:
  - name: corp-ca
    configMap:
      name: corp-root-ca
extraVolumeMounts:
  - name: corp-ca
    mountPath: /etc/perf-sentinel/ca
    readOnly: true
```

## Fichier d'acknowledgments

`.perf-sentinel-acknowledgments.toml` est un fichier séparé de `.perf-sentinel.toml`. Il vit à la racine du repo applicatif et liste les findings que l'équipe a acceptés comme connus. Les findings acquittés sont retirés de la sortie CLI (`analyze`, `report`, `inspect`, `diff`) et exclus de la quality gate.

Règles de chargement :

- Le chemin par défaut est `./.perf-sentinel-acknowledgments.toml` dans le répertoire courant. À surcharger avec `--acknowledgments <chemin>`.
- Si le fichier n'existe pas, le run est un no-op (pas d'erreur, pas de bruit en sortie).
- `--no-acknowledgments` ignore le fichier complètement (vue d'audit).
- Une coquille dans `signature`, un champ requis manquant, ou un `expires_at` mal formé fait échouer le run de façon visible plutôt que d'élargir silencieusement l'ensemble acquitté.

Entrée minimale :

```toml
[[acknowledged]]
signature = "redundant_sql:order-service:POST__api_orders:cafebabecafebabecafebabecafebabe"
acknowledged_by = "alice@example.com"
acknowledged_at = "2026-05-02"
reason = "Cache invalidation pattern, intentional. See ADR-0042."
```

Le champ `expires_at = "YYYY-MM-DD"` est optionnel. L'omettre rend l'ack permanent. Le définir permet d'imposer une réévaluation périodique : quand la date passe, l'ack cesse de s'appliquer et le finding réapparaît au prochain run CI.

Pas de prise en charge des globs ou wildcards, chaque entrée est comparée à une signature exacte. Les signatures sont émises sur chaque finding dans la sortie JSON, copiez-les dans le fichier plutôt que de recalculer le préfixe SHA-256 à la main.

Pour le workflow complet et la FAQ, voir [`ACKNOWLEDGMENTS-FR.md`](ACKNOWLEDGMENTS-FR.md).
