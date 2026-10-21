# Rapport public périodique

Notes de design pour le pipeline de divulgation publique périodique : schéma (actuel v1.5), agrégateur, validateur, archive daemon, et la sous-commande `disclose`. La doc opérateur vit dans `docs/FR/REPORTING-FR.md`, la chaîne de calcul dans `docs/FR/METHODOLOGY-FR.md`, la référence wire dans `docs/FR/SCHEMA-FR.md`.

## Disposition des modules

```
crates/sentinel-core/src/report/periodic/
  ├── mod.rs        // réexportations
  ├── schema.rs     // types wire v1.5
  ├── errors.rs     // ValidationError, HashError, AggregationError
  ├── hasher.rs     // JSON canonique + SHA-256 + fonction utilitaire binary_hash
  ├── validator.rs  // validate_official, validate_content_hash
  ├── aggregator.rs // lecteur archive NDJSON, attribution par service
  ├── rollup.rs     // format de ligne du cumul quotidien, accumulateur du jour
  └── org_config.rs // chargeur du TOML fourni par l'opérateur

crates/sentinel-core/src/daemon/archive.rs         // écriture de l'archive
crates/sentinel-core/src/daemon/rollup_writer.rs   // écriture des cumuls quotidiens
crates/sentinel-cli/src/disclose.rs                // dispatcher CLI
```

La répartition reprend le pattern pipeline du reste de la crate : fonctions pures sur les données, traits uniquement aux frontières d'I/O (`std::fs` pour l'org-config et l'archive, `tokio::sync::mpsc` pour la tâche writer). Pas de nouvelle abstraction entre les étages.

## Déterminisme du schéma

Le content hash est un SHA-256 sur la forme JSON canonique du rapport avec `integrity.content_hash` mis à chaîne vide. Trois invariants rendent cela reproductible entre builds et entre consommateurs :

1. **L'ordre des champs est celui de la déclaration des structs.** `serde_json` préserve l'ordre des champs lors de la sérialisation. Réorganiser des champs dans `schema.rs` casse le hash et doit donc s'accompagner d'un incrément de version de schéma.
2. **Chaque map est un `BTreeMap`.** `HashMap` itère dans un ordre non déterministe et défait le hash. Le schéma utilise `BTreeMap<String, String>` pour `notes.reference_urls`, et les buffers intermédiaires de l'agrégateur (`per_service`, `anti_patterns`, `first_seen`, `last_seen`) suivent la même discipline.
3. **`Application::G1` et `Application::G2` sont en `#[serde(untagged)]`.** Pas de champ discriminant, l'aiguillage se fait par présence de champ requis (`anti_patterns` pour G1, `anti_patterns_detected_count` pour G2). Le tableau applications est imposé homogène par le validateur, donc le niveau type est permissif mais l'invariant runtime est strict.

L'implémentation du hasher (`hasher.rs`) lance ensuite `canonicalize(Value)` qui reconstruit chaque objet JSON via `BTreeMap<String, Value>` et récurse dans les tableaux. C'est défensif : `serde_json::Map` sans la feature `preserve_order` est déjà un `BTreeMap`, mais le passage explicite garde l'implémentation correcte si une dépendance future active la feature de manière transitive.

La sortie du hash est `"sha256:<64-hex>"`. L'encodage hexadécimal est fait à la main (`{byte:02x}`) pour éviter d'ajouter la crate `hex`, en cohérence avec le pattern existant dans `crate::acknowledgments`.

### Pourquoi vider la valeur plutôt que retirer la clé

Mettre `content_hash` à `""` (chaîne vide) préserve la clé dans la forme canonique. Les consommateurs qui vérifient le hash n'ont pas à savoir s'il faut ajouter ou retirer la clé. Ils remplacent la valeur lue par `""` et recalculent. Le schéma accepte à la fois `^sha256:[0-9a-f]{64}$` et la chaîne vide pour ce champ, ce qui permet aux exemples d'être livrés avec un placeholder.

## Granularité G1 / G2

Les deux granularités existent parce qu'un rapport de transparence publiable ne doit pas exposer le détail par anti-pattern (qui peut se lire comme un runbook des faiblesses) tandis que les brouillons internes en bénéficient. Le validateur impose :

- `confidentiality = "internal"` accepte G1 ou G2.
- `confidentiality = "public"` exige G2.
- Mélanger des entrées G1 et G2 dans le même tableau `applications` est refusé.

Le choix de `#[serde(untagged)]` plutôt qu'un discriminateur explicite a été fait pour ces raisons :

- La discrimination est structurelle (`anti_patterns` vs `anti_patterns_detected_count`) et le JSON Schema sait déjà l'exprimer avec `oneOf` plus des contraintes `not: { required }`.
- Le tableau applications est censé être homogène, donc un consommateur externe parsant le JSON n'a pas à gérer un tableau aux tags mixtes.
- Les appelants Rust internes travaillent aussi en pratique sur une slice homogène, donc le `match` sur `Application::G1(_)` / `Application::G2(_)` reste local à quelques sites du builder CLI.

## Validateur collect-all

`validate_official` retourne `Result<(), Vec<ValidationError>>` et accumule toutes les violations en un seul passage plutôt que de quitter à la première. Raisons :

- Les opérateurs configurant un daemon `intent = "official"` corrigent l'org-config en un aller-retour au lieu de découvrir les champs manquants un par un à chaque redémarrage.
- Les relecteurs face à un échec CLI voient immédiatement la liste complète des problèmes structurels.

La fonction délègue à des helpers par section (`validate_organisation`, `validate_period`, `validate_scope_manifest`, `validate_methodology`, `validate_aggregate`, `validate_applications`). Chaque helper prend `&mut Vec<ValidationError>` et y ajoute ses erreurs. Les sous-règles à l'intérieur d'un helper continuent à s'exécuter après un ajout : par exemple, le helper méthodologie valide chaque entrée de `enabled_patterns` et `core_patterns_required` contre `KNOWN_PATTERNS` même si une entrée plus tôt a déjà été refusée.

`KNOWN_PATTERNS` est un `const &[&str]` dans `validator.rs` qui reflète les variants de `FindingType`. Un test (`known_patterns_matches_finding_type_count`) utilise un match exhaustif sur `FindingType` pour forcer un échec CI si un futur variant est ajouté sans mise à jour de la liste.

`intent = "internal"` est un no-op : un brouillon a le droit d'être incomplet. `intent = "audited"` court-circuite avec un unique `ValidationError::AuditedNotImplemented`, accepté par le JSON schema pour la compatibilité ascendante mais non implémenté au runtime.

## Agrégateur et attribution par service

L'agrégateur lit des fichiers NDJSON (ou des dossiers contenant des `*.ndjson`), où chaque ligne est une enveloppe :

```json
{"ts":"<RFC 3339 UTC>","report":{...full Report...}}
```

Depuis 0.28.0, les entrées peuvent aussi contenir des fichiers de cumuls quotidiens, voir Cumuls quotidiens plus bas.

Pour chaque enveloppe dans la période :

1. **Compteurs globaux** somment `total_io_ops`, `avoidable_io_ops`, `total.mid` (gCO2), `avoidable.mid` (gCO2). gCO2 est divisé par 1000 pour obtenir kgCO2eq.
2. **Distribution par service** lit `Report.per_endpoint_io_ops` pour l'ensemble des services qui ont produit des I/O dans la fenêtre. Chaque service reçoit une part de l'énergie/carbone de la fenêtre proportionnelle à sa part d'I/O ops.
3. **Attribution des findings** parcourt `Report.findings`. Chaque finding est rangé sous son `service` et son `finding_type.as_str()`. `first_seen` et `last_seen` suivent la plage d'horodatages de fenêtre par `(service, pattern_type)`.

Quand une fenêtre a zéro entrée dans `per_endpoint_io_ops`, ses totaux globaux tombent dans le bucket `"_unattributed"` et le bucket apparaît dans le tableau applications. C'est un arbitrage. Ignorer silencieusement la fenêtre gonflerait les parts par service des fenêtres suivantes, et refuser l'exécution sur une seule fenêtre creuse serait trop agressif pour beaucoup de déploiements réels. Le flag `--strict-attribution` (et la variante `AggregationError::UnattributedWindow` associée) est la porte de sortie pour les opérateurs qui préfèrent la posture stricte.

Une fenêtre peut compter dans ses totaux l'énergie mesurée ou déclarée des hôtes de base de données et de broker, marquée `in_totals` sur le chiffre. L'agrégateur tient cette part d'hôte à l'écart des applications. `total_energy_kwh` et `total_carbon_kgco2eq` l'incluent, et `carbon_breakdown.database_kgco2eq` et `messaging_kgco2eq` publient son carbone. Un hôte n'est jamais un service : les maps runtime par service ne portent que l'énergie applicative, et les chemins de répartition par proxy et `_unattributed` répartissent l'énergie de la fenêtre moins l'énergie d'hôte incluse dans les totaux, et le carbone de la fenêtre moins son carbone d'hôte. Quand cette part applicative n'est pas positive, le proxy I/O la chiffre et l'énergie d'hôte est conservée. Un chiffre inclus dans les totaux n'alimente jamais `database_kgco2eq_out_of_total` ni `messaging_kgco2eq_out_of_total`, qui ne reçoivent que les chiffres mesurés ou déclarés qu'une fenêtre a gardés à côté de ses totaux (archivée avant que l'énergie des hôtes y entre, ou sans carbone), et son modèle rejoint `energy_source_models`.

Les lignes malformées (échecs de parse) sont sautées avec un `tracing::warn!` et comptées dans `malformed_lines_skipped`. L'agrégateur ne refuse pas de continuer sur des erreurs de parse isolées. La motivation est l'archive daemon : une ligne partiellement écrite pendant un crash ne doit pas empoisonner toute la période.

## Writer d'archive daemon

Le writer est une tâche `tokio::spawn` alimentée par un `tokio::sync::mpsc::Sender<OwnedArchive>` borné, capacité 256. Côté producteur (dans `process_traces`, sur le worker d'analyse), le code appelle `archive::try_send(tx, OwnedArchive { ts, report })` pour que le chemin de scoring par fenêtre du daemon ne bloque jamais sur l'I/O disque. Envoyer un `OwnedArchive` typé (et non une chaîne pré-sérialisée) sort le coût `serde_json::to_string` du chemin chaud et laisse la tâche writer l'amortir contre l'I/O disque.

Le canal borné applique une politique drop-on-full : quand le writer prend du retard, les nouvelles fenêtres sont jetées avec un `tracing::warn!`. La capacité de 256 messages est dimensionnée pour que l'état d'un writer bloqué en régime permanent remonte en quelques secondes plutôt que de laisser un canal non borné provoquer un OOM du daemon.

La tâche writer effectue des I/O `std::fs` synchrones avec tampon. Par construction, les producteurs ne bloquent jamais dessus (`try_send` en drop-on-full), les écritures ligne par ligne passent par un `BufWriter`, et la rotation s'exécute une fois par tranche de `max_size_mb` écrits. Le pire cas d'un système de fichiers figé est un worker du runtime bloqué dans cette tâche, ce que le canal borné convertit en fenêtres jetées plutôt qu'en contre-pression sur le chemin d'analyse. `tokio::fs` ou `spawn_blocking` ajouteraient un surcoût par ligne sans aucun gain sur le comportement.

La rotation se déclenche quand `bytes_written` dépasse `max_size_mb * 1_048_576`. Le fichier actif est renommé en `<stem>-<UTC-timestamp>.ndjson` d'abord, puis un nouveau fichier est ouvert via `OpenOptions::create_new(true).append(true)` pour fermer la course TOCTOU où un attaquant co-résident pourrait planter un symlink entre le rename et la réouverture. `prune` retire les plus anciens fichiers tournés jusqu'à n'en conserver au plus que `max_files`. Le pruning trie par `mtime` décroissant et valide que le suffixe d'horodatage correspond à la forme `is_rotation_stamp`, ainsi un fichier sans rapport dans le répertoire d'archive (par exemple `archive-evil.ndjson`) n'est jamais supprimé.

`metadata_len` lit la taille du fichier existant au démarrage pour que le writer reprenne correctement après un redémarrage du daemon sans tourner immédiatement un fichier presque plein.

### Pourquoi archiver des `Report` plutôt que des findings

L'agrégateur a besoin de `green_summary` (pour énergie/carbone) et de `per_endpoint_io_ops` (pour l'attribution par service). Un flux de `findings` seul ne porte pas ces données. Le daemon construit un `Report` depuis `findings + green_summary + per_endpoint_io_ops + analysis` juste après `emit_findings_and_update_metrics`, puis envoie l'enveloppe sérialisée. Le coût est un `Vec<Finding>::clone` et un `serde_json::to_string` par fenêtre quand l'archive est activée.

Comme `process_traces` reçoit déjà la valeur `per_endpoint_io_ops` de `score_green`, la garder pour l'archive n'ajoute aucun coût au chemin chaud.

### Palier évitable canonique à l'archivage (1.1+)

Le `n_plus_one_threshold` de l'opérateur décide quels patterns N+1 deviennent des findings, donc un seuil relâché réduit l'énergie/carbone évitable que la divulgation déclarerait. `disclose` ne fait que sommer des chiffres déjà archivés et ne peut pas re-détecter (les findings supprimés par un seuil élevé sont absents de l'archive). Le chiffre non manipulable doit donc être produit là où les traces brutes existent encore : le chemin d'archivage du daemon.

`score::canonical::compute_disclosure_waste` exécute une passe N+1 + redondant supplémentaire au seuil épinglé dans le binaire `DISCLOSURE_N_PLUS_ONE_THRESHOLD` (`2`) et remet à l'échelle l'énergie/carbone évitable depuis `operational_gco2` et `accounted_io_ops` du résumé opérationnel (pas de second calcul carbone complet). Il renvoie les deux paliers, archivés sur `Report.disclosure_waste` : `canonical` au seuil épinglé et `operational` à celui de l'opérateur. Le tableau de bord live et `findings_store` gardent la sémantique opérationnelle, donc seule l'archive de divulgation porte le palier canonique. L'agrégateur regroupe les deux paliers dans `aggregate.canonical_waste` / `operational_waste`, les champs plats évitables étant des alias du palier canonique. La passe supplémentaire n'est payée que quand l'archivage est activé.

Une amélioration différée estamperait le seuil canonique par fenêtre et réconcilierait à travers un parc de binaires hétérogène à l'agrégation. Aujourd'hui l'agrégateur réconcilie les seuils par `max` et expose les binaires producteurs via `aggregate.binary_versions`.

Le validateur authentifie le *label* canonique (`canonical_waste.n_plus_one_threshold == 2`), pas la *magnitude* des chiffres archivés : une ligne NDJSON falsifiée peut porter le seuil 2 avec des compteurs dégonflés et passer quand même. C'est inhérent à un modèle d'auto-déclaration. Le `content_hash` (et l'attestation cosign optionnelle) lie l'intégrité du *rapport publié*. La fiabilité des archives sources repose sur le `binary_hash` du binaire et la provenance SLSA, pas sur l'agrégateur. Les compteurs issus des archives sont sommés avec `saturating_add`, pour qu'une valeur proche de `u64::MAX` forgée ne puisse pas faire déborder un total de période vers un petit nombre (sous-déclaré).

## Cumuls quotidiens (0.28.0)

L'archive de fenêtres écrit un `Report` complet par fenêtre de scoring, toutes les `trace_ttl_ms / 2` (15 s par défaut). Une mesure en laboratoire sous charge synthétique a relevé 350 à 900 Kio par ligne, donc les 12 fichiers de 100 Mio par défaut tiennent 6 à 16 heures, jamais un trimestre. La 0.28.0 a fait refuser à un `disclose` officiel une période que l'archive n'atteint plus, ce qui empêche un rapport faux sans faire tenir la période dans l'archive.

Avec `[daemon.archive]` configurée, la tâche d'écriture produit désormais aussi des cumuls quotidiens : une ligne NDJSON par jour UTC dans `<stem>-daily-YYYY-MM.ndjson`, dans le répertoire de `[daemon.archive] path`, un fichier par mois. Avec `path = "/var/lib/perf-sentinel/archive.ndjson"`, octobre 2026 arrive dans `archive-daily-2026-10.ndjson` à côté. Une ligne contient l'état partiel propre à l'agrégateur pour les fenêtres de ce jour, donc un jour coûte quelques dizaines de Kio et un trimestre quelques Mo au plus.

### Agrégation partagée

`PeriodState` dans `aggregator.rs` est la moitié mesurante de l'agrégateur : ce que les fenêtres d'une période ont mesuré, indépendamment de la façon dont elles ont été lues. `process_window` y agrège une fenêtre et `merge` y fusionne un autre état. `Builder` ne garde que ce qui décrit la lecture : les compteurs de chaîne, les valeurs de base du compteur de fenêtres jetées, la plus ancienne fenêtre archivée et les jours qu'un cumul a déjà couverts. Le `DayAccumulator` du daemon (`rollup.rs`) exécute le même `process_window` sur chaque fenêtre, donc une ligne de cumul porte l'état que `disclose` construirait depuis les lignes de fenêtre de ce jour.

Chaque champ est additif : il fusionne par l'opération qu'utilise son agrégation fenêtre par fenêtre, et chaque opération est commutative et associative.

- Sommes : requêtes, I/O ops, énergie, termes carbone, nombres de fenêtres, chiffres évitables par palier, gaspillage base de données et messagerie.
- Accumulateurs par service : eux-mêmes des sommes et des unions d'ensembles.
- Unions d'ensembles : modèles d'énergie, versions de binaire, méthodologies carbone, coefficients de scoring, modèles d'énergie par service, jours observés.
- Minimum et maximum : `first_seen` et `last_seen` par `(service, pattern)`, le seuil N+1 des paliers.
- Ou logique : `calibration_applied`, `transport_coefficient_uncertain`.
- Paires somme et compte : le ratio mesuré par service, transformé en moyenne seulement dans `finalize`, pour que les jours qui ont plus de fenêtres gardent leur poids.

L'attribution par service a lieu dans l'agrégation de chaque fenêtre, donc un état fusionné n'a pas besoin d'une seconde attribution. Plusieurs lignes d'un même jour s'additionnent, et l'état fusionné se finalise sur les chiffres de l'agrégation fenêtre par fenêtre aux arrondis près (voir Arrondis plus bas).

Une ligne de cumul est lue depuis un fichier, donc `merge` réapplique chaque garde-fou du chemin des fenêtres : le plafond de 4096 services (`MAX_SERVICES`), des noms d'anti-pattern limités aux valeurs connues de `FindingType`, les chiffres non finis ou négatifs ramenés à zéro (`sanitize_f64`), le contrôle de jeu de caractères et de longueur de `binary_version`, les contrôles des étiquettes de modèle d'énergie et de provenance, et `saturating_add` sur les compteurs. Une entrée `first_seen` ou `last_seen` ne survit que si sa paire `(service, pattern)` a survécu à la fusion des services. Un cumul ne répond en outre que pour son propre jour : `keep_only_day` restreint ses jours observés au `day` de la ligne avant la fusion, donc une ligne forgée ne peut pas relever `temporal_coverage` sur d'autres jours.

### Format de ligne et chaîne

Une ligne de cumul est un objet JSON avec ces champs :

- `format` : `"perf-sentinel-rollup/v1"`, dans le corps haché. Le lecteur reconnaît le contenu d'un cumul à ce champ, et la version permet à l'agrégation d'évoluer.
- `ts` : la première fenêtre du jour, pour que la vérification de la 0.28.0 qui exige que l'archive remonte au début de la période (`archive_reaches_period_start`) le lise comme le `ts` d'une fenêtre.
- `day` : le jour UTC, qui décide si la ligne tombe dans la période.
- `last_ts` : la dernière fenêtre du jour. `disclose` ne le lit pas.
- `state` : le `PeriodState` sérialisé.
- `prev`, `seq` : le maillon de chaîne, comme sur les lignes de fenêtre.
- `drops` : le compteur cumulé de fenêtres jetées, comme sur les lignes de fenêtre.
- `hash` : le même hash SHA-256 de chaîne d'archive que les lignes de fenêtre (`archive_chain_hash`), calculé sur la ligne privée de son champ `hash`.

Tous les cumuls d'une archive forment une seule chaîne, jamais réinitialisée : elle court à travers les fichiers mensuels et à travers les redémarrages. Au démarrage, la tâche reprend `prev` et `seq` depuis la dernière ligne du fichier mensuel le plus récent, ou part de `ARCHIVE_CHAIN_SEED` s'il n'y en a aucun. Un jour supprimé ou modifié apparaît donc comme une rupture de chaîne, contrairement à un fichier de fenêtres purgé, dont la chaîne repart à chaque fichier tourné et ne laisse aucune trace. Le lecteur garde une ancre par famille de cumuls (répertoire et `<stem>`) à travers les fichiers mensuels, lus dans l'ordre de leurs noms, et adopte la plus ancienne ligne restante comme début de chaîne, donc la rétention n'est pas une rupture. Dans les compteurs d'intégrité publiés, une ligne de cumul vérifiée compte comme vérifiées les fenêtres qu'elle contient, et une ligne de cumul rompue compte une rupture.

### Clôture d'un jour

Un jour se clôt quand arrive la première fenêtre d'un autre jour UTC, selon le `ts` de cette fenêtre et non l'horloge murale, ainsi qu'à l'arrêt propre. L'archivage est déclenché par le trafic, donc sur un déploiement calme un jour ne se clôt qu'à la reprise du trafic. Un daemon redémarré le même jour écrit une seconde ligne pour ce jour, et une horloge reculée par-delà minuit écrit une seconde ligne pour le jour antérieur. Les lignes d'un même jour s'additionnent, rien n'est écrasé. Quand la sérialisation ou l'écriture d'une ligne échoue, un avertissement est journalisé et le jour est gardé en mémoire, puis retenté dans l'ordre à la clôture de jour suivante, jusqu'à 62 jours. À l'arrêt, le jour ouvert part dans l'instantané au lieu d'une ligne que le répertoire refuse, et les jours encore refusés sont journalisés en erreur et perdus pour les cumuls. Leurs lignes de fenêtre comptent encore, puisqu'aucun intervalle de cumul ne les couvre. Avant d'ajouter une ligne, la tâche scelle sur sa propre ligne tout fragment qu'un arrêt brutal a laissé en fin de fichier, et coupe ce qu'une écriture ratée a déposé, comme le fait l'écriture des fenêtres. Une ligne n'est jamais rangée dans un mois plus ancien que le fichier le plus récent sur disque, donc une horloge reculée au-delà d'un début de mois garde une seule chaîne ordonnée.

### Instantané en cas d'arrêt brutal

Toutes les 20 fenêtres (`SNAPSHOT_EVERY`, environ cinq minutes avec la fenêtre par défaut), le jour ouvert est écrit dans `<stem>-daily.partial.json` : un fichier temporaire, réservé au propriétaire et ouvert sans suivre les liens symboliques, synchronisé, puis renommé par-dessus l'instantané. Au démarrage, la tâche reprend la chaîne depuis la dernière ligne du mois non vide le plus récent, donc un fichier récent vide ne masque pas la tête de chaîne. Elle ajoute ensuite l'instantané une seule fois comme ligne, sauf si une ligne sur disque part déjà de la même première fenêtre (même `ts`), ce qui arrive quand un arrêt brutal tombe après l'écriture de la ligne d'un jour et avant que l'instantané soit remplacé ou supprimé, puis le supprime. Si cet ajout échoue, l'instantané reste sur disque et le jour rejoint la liste des reprises. Un instantané qui est un lien symbolique est refusé avec un avertissement. Un arrêt brutal (SIGKILL, OOM) perd donc au plus les 20 dernières fenêtres des cumuls, jamais le jour, et ne compte jamais un jour deux fois. L'instantané ne porte ni étiquette `format` ni champ de chaîne, il ne peut donc jamais passer pour une ligne de cumul.

### Rétention

`[daemon.archive] rollup_retention_days` (défaut 400, un peu plus d'un an, au moins 1) fixe une rétention par âge. À chaque clôture de jour, chaque fichier mensuel dont le dernier jour précède de plus de ce nombre de jours le jour qui vient de se clore, ou la date du jour quand ce jour est dans le futur, est supprimé en entier, donc un fichier chaîné n'est jamais réécrit et une seule fenêtre datée dans le futur ne peut pas effacer l'historique. La soustraction est vérifiée, donc aucune valeur de rétention ne déborde. La rotation des fenêtres (`max_size_mb`, `max_files`) ne touche jamais aux fichiers de cumuls.

`[daemon.archive] windows = false` arrête les lignes par fenêtre et ne garde que les cumuls. Le défaut est `true` : les lignes de fenêtre restent la source du détail fenêtre par fenêtre et ce qu'interroge le guide d'exploitation.

### Lecture des cumuls par `disclose`

`disclose --input` accepte des fichiers de fenêtres, des fichiers de cumuls, ou les deux, et un répertoire qui contient les deux. `is_rollup_file` tranche sur le contenu : un fichier dont la première ligne lisible porte l'étiquette de format est un fichier de cumuls, un fragment d'arrêt brutal placé avant ne le déclasse pas, donc un fichier de cumuls renommé ou copié est toujours lu comme tel. L'instantané `.json` n'est jamais pris dans un répertoire.

Les fichiers de cumuls sont lus en premier (`process_rollup_file`). Chaque ligne de cumul de la période note l'intervalle des fenêtres qu'elle a agrégées, de `ts` à `last_ts`, sous son propriétaire : la famille de fenêtres que `archive_family` donne au fichier de fenêtres voisin, même répertoire et même radical. Une ligne de fenêtre de cette famille comprise dans un intervalle noté n'alimente que la vérification de chaîne et le compteur de fenêtres jetées (`rollup_covers`). Les lignes de fenêtre hors de tout intervalle comptent encore, comme les fenêtres antérieures du jour de la mise à jour et celles qu'un arrêt brutal a fait perdre au cumul après son dernier instantané, et les cumuls d'un hôte ne masquent jamais les fenêtres d'un autre. La chaîne d'une famille de cumuls démarre sans ancre, puisque la rétention retire des mois par le début, mais une ligne sans hash compte comme une rupture dès la première ligne, puisque les cumuls ont toujours été chaînés.

Le daemon agrège sans `--strict-attribution`, qui reste le choix du lecteur. L'état garde la plus ancienne fenêtre tombée dans `_unattributed` (`first_unattributed`), donc avec cette option `disclose` refuse un cumul de la période qui en a agrégé une, et nomme cette fenêtre.

La vérification de la 0.28.0, qui exige que l'archive remonte au début de la période, prend le plus ancien `ts` vérifié parmi les lignes de cumul et de fenêtre. Avec les cumuls, une divulgation trimestrielle officielle fonctionne avec les réglages par défaut dès que le daemon tourne depuis le premier jour du trimestre.

### Arrondis

Les chiffres issus des cumuls correspondent à ceux des mêmes fenêtres aux arrondis flottants près : un cumul somme d'abord un jour, donc les additions se font dans un autre ordre. Les tests d'équivalence bornent l'écart relatif à 1e-9. La même période divulguée depuis les cumuls et depuis les fenêtres peut donc différer sur ses derniers chiffres, et donc sur son `content_hash`. Divulguer deux fois les mêmes entrées reste reproductible.

### Ce que voit l'opérateur

- `/api/status` gagne `disclosure_archive: {"oldest_day": "YYYY-MM-DD", "days": N, "last_closed_day": "YYYY-MM-DD"}`, absent tant qu'aucun jour n'est clos. Additif depuis 0.28.0.
- `/metrics` gagne trois jauges sans étiquette : `perf_sentinel_archive_oldest_day_timestamp_seconds` (plus ancien jour conservé, secondes epoch à 00:00 UTC), `perf_sentinel_archive_rollup_days` (jours distincts conservés) et `perf_sentinel_archive_last_closed_day_timestamp_seconds` (jour clos le plus récent). Toutes valent 0 tant qu'aucun jour n'est clos. Elles sont posées au démarrage depuis les fichiers sur disque et après chaque jour clos.
- Le schéma publié `PeriodicReport` ne change pas : les cumuls sont seulement un format d'entrée.
- Le chart Helm ne change pas : les cumuls arrivent à côté du chemin d'archive, sur le volume persistant quand la persistance est activée.

## TOML org-config

Le TOML fourni par l'opérateur est un blueprint partiel pour les champs statiques d'un `PeriodicReport`. Il porte `organisation`, `methodology`, `scope_manifest` (sans les chiffres runtime) et `notes` optionnel. L'agrégateur remplit les sections runtime (`aggregate`, `applications`, `integrity`).

`load_from_path` retourne `OrgConfig` ou `OrgConfigError` (`Io` ou `Parse`). `validate_for_official` retourne `Vec<String>` plutôt que des erreurs typées parce que le daemon les aplatit dans `DaemonError::ReportingValidation { errors: Vec<String> }` pour des logs de démarrage lisibles. La sous-commande `disclose` côté CLI appelle la version typée `validate_official` sur le rapport entièrement assemblé, ce qui lui permet de remonter aussi les violations au niveau agrégat (par exemple `applications` vide, ratio hors plage).

Les champs TOML reflètent verbatim le schéma wire. Un opérateur qui lit le JSON Schema peut écrire le TOML sans consulter un deuxième document, et un mainteneur qui renomme un champ wire doit le renommer aux deux endroits.

## Garde-fou au démarrage du daemon

`daemon::run` appelle `validate_official_reporting` avant d'allouer la moindre ressource. Le helper :

1. Retourne `Ok` quand `[reporting] intent != "official"`.
2. Charge l'org-config depuis `[reporting] org_config_path`. Chemin manquant ou fichier illisible devient une entrée dans le vec d'erreurs.
3. Appelle `org_config::validate_for_official` et fusionne son `Vec<String>` dans le même vec.
4. Retourne `Err(DaemonError::ReportingValidation { errors })` si quoi que ce soit échoue, avec un `Display` qui produit une ligne indentée par erreur pour que journalctl / kubectl logs rendent proprement.

Les listeners ne démarrent pas quand la validation échoue, et le daemon sort avec un statut non zéro. Les opérateurs qui préfèrent un mode souple fixent `intent = "internal"` (ou omettent la section).

## Dispatcher CLI

`Commands::Disclose` a été choisi plutôt qu'une extension de `Commands::Report` existant pour ne pas casser la surface CLI (`Report` est déjà la sous-commande du dashboard HTML/JSON). Le verbe `disclose` correspond au vocabulaire opérateur de publication de transparence et se lit bien dans des scripts shell.

Le dispatcher (`disclose.rs::cmd_disclose`) retourne `i32` pour que l'appelant puisse faire `std::process::exit(code)` directement. Le contrat :

- `0` : succès, fichier écrit.
- `1` : échec I/O ou parse (org-config illisible, output non écrivable, erreur hash).
- `2` : échec de validation ou court-circuit `audited`. La liste d'erreurs est imprimée sur stderr.

`audited` est intercepté en premier, avant toute I/O, pour que l'utilisateur reçoive le message "not yet implemented" quel que soit l'état de l'org-config.

`generated_by` vaut `"ci"` quand `$CI` est dans l'environnement, `"cli-batch"` sinon. Le chemin daemon utilisera `"daemon"` quand les divulgations planifiées seront ajoutées. C'est un placeholder pour les trois valeurs documentées du champ.

## Commandes de vérification

Un consommateur recalcule le content hash avec :

```bash
jq -c '.integrity.content_hash = ""' perf-sentinel-report.json \
  | jq -cS '.' \
  | shasum -a 256
```

L'étape `jq -cS` canonicalise les clés d'objet via le flag `S` intégré à jq, ce qui correspond à l'étape `canonicalize` de `hasher.rs`. Le formatage des nombres peut différer sur des entrées avec des représentations JSON non par défaut des flottants. Le schéma n'utilise que des `f64` que `serde_json` émet sous la forme la plus courte qui round-trip, ce qui est aussi ce que jq émet, donc en pratique les deux produisent les mêmes octets.

## Hooks de configuration

Deux nouvelles sections dans `.perf-sentinel.toml` :

- `[reporting]` porte `intent`, `confidentiality_level`, `org_config_path`, `disclose_output_path`, `disclose_period`. Validée au chargement de la config.
- `[daemon.archive]` porte `path`, `max_size_mb` (défaut 100), `max_files` (défaut 12), et depuis 0.28.0 `windows` (défaut `true`) et `rollup_retention_days` (défaut 400, au moins 1). Validée au chargement de la config et à l'ouverture d'archive.

Les deux sections sont optionnelles. Leur absence laisse Perf Sentinel dans son comportement antérieur : NDJSON sur stdout, pas d'archive, pas de garde-fou de reporting.

## Limitations v1.0 portées en disclaimers

- **Énergie + carbone par service runtime-calibrated quand l'archive les porte.** `Builder::process_window` lit `green_summary.energy_kwh` et les maps `per_service_carbon_kgco2eq` / `per_service_energy_kwh` / `per_service_region` de la fenêtre source quand elles sont peuplées, et se rabat sur le proxy I/O + part proportionnelle quand elles ne le sont pas (archives proxy-only). L'agrégateur expose les tags `energy_model` observés sous `methodology.calibration_inputs.energy_source_models`. Voir `docs/FR/design/09-CARBON-ATTRIBUTION-FR.md`.
- **Le potentiel d'optimisation exclut l'embarqué.** `estimated_optimization_potential_kgco2eq` ne somme que `co2.avoidable.mid`. `total_carbon_kgco2eq` est le `co2.total.mid` complet (opérationnel + embarqué). Les disclaimers par défaut le précisent.
- **`_unattributed` co-route les findings.** Une fenêtre sans `per_endpoint_io_ops` et sans maps runtime per-service range son énergie/carbone ET ses findings sous `_unattributed`. Sans ce routage, un service avec des findings N+1 pourrait être publié à `efficiency_score = 100` si son `total_io_ops` se trouve à zéro dans la même fenêtre.

## Le seuil de 75% de calibration runtime

La constante `MIN_PERIOD_COVERAGE_FOR_OFFICIAL` dans `report::periodic::validator` conditionne une divulgation d'intent `official` à `period_coverage >= 0.75`. Les rapports en deçà sont refusés avec un message qui invite l'opérateur à raccourcir la période ou à se rabattre sur `intent = internal`.

### Pourquoi 75% et pas une autre valeur

Le seuil équilibre deux modes de défaillance.

- **Trop strict** (par exemple 95%) : rejette les migrations légitimes. Un opérateur qui déploie Scaphandre en milieu de trimestre ne produirait jamais un rapport officiel pour ce trimestre, même si trois quarts des données sont correctement calibrées.
- **Trop permissif** (par exemple 50%) : autorise des rapports où la moitié des données vient du repli proxy. L'énergie agrégée et l'attribution par service sous-estimeraient ou déformeraient silencieusement le total de la période pour la moitié des fenêtres.

### Justification empirique

Le choix de 75% reflète trois observations.

- Une migration opérateur typique (déploiement de Scaphandre sur une flotte, passage de on-prem à cloud SPECpower, redéploiement du daemon avec une nouvelle config) prend une à deux semaines. Sur un trimestre civil de 90 jours, cela représente 11 à 22% de la période. Un seuil à 75% accommode ce type de migration sans rejeter le rapport produit.
- En dessous de 75%, le repli proxy contribue à plus d'un quart de l'estimation énergétique totale. Le proxy est uniforme entre services et régions, sa part dilue donc à la fois le total runtime-calibrated et l'attribution par service. Un rapport où le proxy porte plus d'un quart du signal n'est pas fidèlement décrit comme "runtime-calibrated".
- Le seuil de 75% s'aligne sur `MIN_DAYS_COVERED = 30` de manière heuristique. Sur un trimestre, une fenêtre de 30 jours avec couverture complète représente un tiers de la période. Combiné à l'exigence que le reste soit majoritairement calibré pour rester au-dessus de 75%, le gate dessine une forme cohérente de "assez de données, assez de calibration".

### Quand le reconsidérer

Ce seuil n'est pas normatif. Il doit être ajusté si un retour terrain d'opérateurs ou d'auditeurs montre qu'il est trop strict (rapports `internal` qui atterrissent régulièrement juste sous 75% et qui auraient été utiles en `official`) ou trop permissif (un audit qui établit qu'un quart de proxy suffit à masquer une régression). La constante vit dans `crates/sentinel-core/src/report/periodic/validator.rs` et est ré-exportée via le module `report::periodic`.

## Couverture temporelle (v1.2)

`period_coverage` (ci-dessus) répond à "quelle part de la période était runtime-calibrated", pas à "quelle part de la période a été mesurée tout court". Les deux sont indépendants : un daemon qui n'a tourné que trois jours sur 90 déclarés peut quand même rapporter `period_coverage = 1.0` si ces trois jours étaient pleinement calibrés. Rien dans le schéma v1.1 ne révélait ce trou. `days_covered` est de l'arithmétique calendaire pure (`(to - from) + 1`), donc il décrit la fenêtre déclarée par l'opérateur, pas l'activité réelle du daemon.

`aggregate.temporal_coverage` comble ce trou. L'agrégateur suit l'ensemble des jours calendaires UTC distincts portant au moins une fenêtre agrégée (`Builder.observed_days`, inséré dans `process_window` juste après que la fenêtre est validée, pour rester aligné avec `windows_aggregated`). `finalize` divise ce décompte par `period.days_covered` et enregistre aussi `observed_days`, `days_in_period` et `largest_gap_days` (la plus longue suite de jours consécutifs de la période sans fenêtre).

### Pourquoi un avertissement publié, pas une barrière

L'archivage du daemon est **déclenché par le trafic**, pas par une minuterie. `process_traces` retourne tôt sur un lot vide et le `try_send` d'archive est placé après cette vérification, donc une fenêtre sans trafic n'écrit aucune ligne NDJSON. Par conséquent `temporal_coverage` mesure les *jours avec trafic observé*, une borne basse de l'activité, pas l'uptime du daemon. Les jours légitimement calmes (nuits, week-ends, services peu sollicités, un service sans requête un jour férié) l'abaissent. Une barrière dure `official` rejetterait donc des rapports légitimes de déploiements intermittents ou peu sollicités. Donc `validate_official` ne fait que vérifier la plage du champ (`[0, 1]`, fini) et ne bloque jamais dessus. La CLI `disclose` publie la valeur, affiche un avertissement sur stderr sous `LOW_TEMPORAL_COVERAGE_WARN_THRESHOLD`, et ajoute un disclaimer en bande (couvert par le hash) portant la mise en garde sur le déclenchement par le trafic. Le lecteur juge.

### Ce qu'il traite et ce qu'il ne traite pas

C'est le signal interne au binaire le plus proche de l'échappatoire d'auto-déclaration "il suffit d'arrêter Perf Sentinel une partie de la période". L'extinction partielle se voit maintenant comme un `temporal_coverage` bas et un `largest_gap_days` grand. Il ne traite **pas** la non-participation totale (ne jamais lancer l'outil ne laisse aucun rapport) ni un dénominateur malhonnête (`total_requests_in_period` fixé bas), tous deux irréductibles sans infrastructure externe, voir Révisions futures. Deux vérifications de cohérence bon marché l'accompagnent : `days_covered` doit valoir `(to_date - from_date) + 1` (rejet dur, seul un fichier édité à la main peut échouer) et `requests_measured` ne doit pas dépasser un `total_requests_in_period` déclaré par l'opérateur (rejet dur).

## Crosswalk standard et critères RGESN (v1.3)

La v1.3 ajoute deux champs de correspondance interprétative, dont aucun n'est une barrière. `methodology.standard_crosswalk` est un crosswalk vers les datapoints ESRS E1 : une aide de correspondance qui pointe chaque chiffre de la divulgation vers le datapoint CSRD / ESRS E1 le plus proche, avec un disclaimer explicite indiquant qu'il ne remplace pas un inventaire audité. `applications[].anti_patterns[].rgesn_criteria` tague chaque anti-pattern détecté avec les critères RGESN 2024 (Référentiel général d'écoconception de services numériques) auxquels il se rattache, pour qu'un auditeur écoconception puisse relier un finding au référentiel. Les deux sont des extensions additives `#[serde(default)]` : les lecteurs plus anciens les ignorent, et un rapport écrit sans eux se rehashe à l'identique. La référence wire des deux vit dans `docs/FR/SCHEMA-FR.md`.

## Blocs de gaspillage par workload (v1.4, v1.5)

La v1.4 ajoute `aggregate.database_waste` et la v1.5 son jumeau messaging `aggregate.messaging_waste`, aux côtés des trois valeurs messaging de `PatternName`. Les deux sont des ventilations informatives, jamais ajoutées par-dessus un total : leur part estimée est déjà une part des totaux, et leur part mesurée ou déclarée est comprise dedans quand la fenêtre l'a comptée (`in_totals`), à côté d'eux sous `*_kgco2eq_out_of_total` sinon. L'ajout des blocs n'a changé le sens d'aucun chiffre existant.

**Un type, deux champs.** `MessagingWasteAggregate` est un alias de type de `DatabaseWasteAggregate`, pas une copie : les deux blocs sont identiques sur le fil, une seule structure supprime donc la possibilité d'une dérive. Le schéma JSON reflète cela par un `$ref` plutôt qu'une `$def` dupliquée, car une définition recopiée à la main dans le fichier que les consommateurs valident réintroduirait la dérive que l'alias empêche. Le nom `DatabaseWasteAggregate` est conservé pour les lecteurs v1.4.

**Quatre seaux de provenance, pas deux.** L'agrégation répartit selon l'étiquette `model` entre `measured_*`, `modeled_*`, `declared_*` et `estimated_windows`, avec `measured_windows + modeled_windows + declared_windows + estimated_windows == windows_with_figure` imposé par le validateur. Déclaré correspond à `is_declared_model` (`database_specpower`, `broker_specpower`), modélisé à `cloud_specpower` (une enveloppe SPECpower interpolée à une lecture CPU), estimé à `estimated`, et toute autre étiquette compte comme mesurée. La forme à deux seaux livrée d'abord (tout ce qui n'est pas `estimated` compte comme mesuré) plaçait un cluster provisionné déclaré dans un champ nommé `measured_energy_kwh`, sur la seule surface où la provenance est tout l'enjeu, et un chiffre interpolé à partir du CPU y aurait atterri de la même façon. `models` prouve quelles étiquettes sont apparues mais couvre toute la période, il ne peut donc pas re-séparer l'énergie. Les paires déclarée et modélisée portent `skip_serializing_if`, un rapport sans aucune de ces sources garde donc la forme d'octets exacte qu'il avait avant l'existence des champs et se re-hashe sur son `content_hash` d'origine.

**La part incluse dans les totaux et les placements (v1.8).** `in_totals_energy_kwh` et `in_totals_windows` additionnent les blocs qu'une fenêtre a comptés dans ses totaux, une ventilation de `total_energy_kwh` et jamais un terme à ajouter. Un bloc `estimated` n'y compte jamais, quel que soit son drapeau, le validateur peut donc borner `in_totals_windows` par les fenêtres mesurées, modélisées et déclarées, et `in_totals_energy_kwh` par `energy_kwh`. `placements` rassemble le `placement` de chaque bloc archivé, une déclaration de l'opérateur. Une ligne de rollup est lue depuis un fichier, `merge` ne garde donc que `dedicated` et `shared`. Les trois sont omis quand ils sont nuls ou vides.

**Invariants préservés par l'agrégation.** Une étiquette de provenance hors du charset de `is_valid_model_tag` fait tomber le bloc entier avant qu'aucun chiffre n'atteigne les sommes, plutôt que d'en publier un partiel. `None` et `0.0` restent distincts sur les jambes carbone : une conversion absente ne doit jamais se lire comme une affirmation de zéro carbone. `windows_with_carbon <= windows_with_figure` signale une image carbone partielle. Le validateur tolère une répartition entièrement nulle, ce à quoi ressemble un rapport antérieur aux champs.

## Paliers canoniques : rendre le chiffre public non manipulable

`score/canonical.rs` existe à cause d'un conflit d'intérêts intégré à l'outil. L'opérateur règle `n_plus_one_threshold`, qui décide combien de répétitions deviennent un finding. L'augmenter est un réglage légitime en CI, où le but est un rapport signal sur bruit sur lequel une équipe agira. Mais l'énergie et le carbone évitables qu'une divulgation publie dérivent de ces mêmes findings : le bouton qui calme la CI réduit aussi le chiffre que l'organisation divulgue.

Chaque fenêtre archivée porte donc **deux** paliers d'évitable. Le palier opérationnel utilise le seuil configuré par l'opérateur et alimente la CLI, la porte et les tableaux de bord. Le palier canonique utilise `DISCLOSURE_N_PLUS_ONE_THRESHOLD`, figé dans le binaire, et alimente la divulgation. Tourner le bouton déplace le premier et ne peut pas déplacer le second.

Il en découle trois conséquences.

- **Les deux paliers sont calculés ensemble, au moment du scoring, dans le daemon.** Ils ne peuvent pas être recalculés plus tard : `disclose` lit des paliers pré-calculés dans l'archive, car les reconstruire demanderait les spans, disparus depuis longtemps. Une fenêtre archivée sans palier canonique n'est pas intégrée du tout à la divulgation, plutôt qu'intégrée au seuil de l'opérateur, pour qu'une archive de millésimes mélangés ne puisse pas confondre les deux en silence.
- **Le chiffre canonique n'est pas "le bon" et l'opérationnel "le faux".** Ils répondent à deux questions différentes : ce que cette équipe a décidé de poursuivre, contre ce que compte un étalon public fixe. La divulgation porte le seuil qui a produit chaque palier, la différence est donc auditable plutôt qu'implicite.
- **Les chiffres de gaspillage se remettent à l'échelle depuis une base indépendante du ratio.** `messaging_waste` et `database_waste` calculent leur jambe canonique depuis l'`energy_gco2` de la fenêtre et non depuis le gaspillage opérationnel, pour qu'un seuil opérateur qui annule le chiffre opérationnel ne puisse pas annuler le carbone canonique.

Les invariants anti-triche sont testés sous la feature `daemon`, seul mode qui produit les deux paliers.

## Révisions futures

- **Signature Sigstore** : `integrity.signature` est réservé. Ajouter une vraie signature est un incrément mineur SemVer du schéma (champ additif passant non null dans certains fichiers).
- **Intent `audited`** : la troisième valeur d'intent demandera une attestation d'audit externe. La forme vivra sous `integrity` ou dans une section voisine (pas encore tranché).
- **Chaîne d'intégrité de traces** : `integrity.trace_integrity_chain` est réservé pour une racine de Merkle sur les traces sources alimentant la divulgation. Hors du périmètre du schéma actuel.
- **Journal inter-périodes** : `integrity.cross_period_log` (réservé en v1.2) est le point d'extension pour un journal externe en ajout seul ou de type Rekor chaînant les `content_hash` successifs entre périodes. C'est ce qui rend la non-participation totale (un opérateur qui arrête de publier) détectable par un tiers, le trou qu'aucune garantie d'intégrité par rapport ne peut combler. Il ne sera renseigné que sous `intent = "audited"`. Comme c'est du contenu divulgué (toujours `None` aujourd'hui, omis sur le fil), il n'est pas dans `POST_SIGN_FIELDS`, donc les hashs des rapports actuels ne changent pas.
- **Intégration Boavizta** : `methodology.calibration_inputs` gagnera un champ `boavizta_version` quand l'intégration sera livrée. Les consommateurs de schéma doivent tolérer des champs de calibration inconnus, ce qu'ils font déjà parce que `additionalProperties` n'est pas posé.

## Mapping des fichiers source

| Fichier source                                       | Sujet                                          |
|------------------------------------------------------|------------------------------------------------|
| `report/periodic/schema.rs`                          | types wire, invariants de déterminisme         |
| `report/periodic/hasher.rs`                          | JSON canonique + SHA-256, binary hash          |
| `report/periodic/validator.rs`                       | validateur collect-all, KNOWN_PATTERNS         |
| `report/periodic/aggregator.rs`                      | folding NDJSON, attribution par service        |
| `report/periodic/rollup.rs`                          | ligne de cumul quotidien, accumulateur du jour |
| `report/periodic/org_config.rs`                      | chargeur du TOML opérateur                     |
| `report/periodic/errors.rs`                          | enums d'erreur                                 |
| `daemon/archive.rs`                                  | writer NDJSON non bloquant avec rotation/prune |
| `daemon/rollup_writer.rs`                            | cumuls quotidiens : chaîne, instantané, purge  |
| `daemon/mod.rs` (`validate_official_reporting`)      | garde-fou de démarrage                         |
| `daemon/event_loop.rs`                               | hook archive dans `process_traces`             |
| `config/` (`ReportingConfig`, `DaemonArchiveConfig`) | sections TOML + validateurs                    |
| `sentinel-cli/src/disclose.rs`                       | dispatcher CLI, value enums, build_report      |
