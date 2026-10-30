# Référence CLI

Cette page documente les sous-commandes utilisateur du binaire
`perf-sentinel`. Pour des notes d'architecture et de design plus
approfondies, voir [`ARCHITECTURE-FR.md`](./ARCHITECTURE-FR.md). Pour
les hooks d'exécution (quality gates CI, codes de sortie, variables
d'environnement), voir [`CI-FR.md`](./CI-FR.md) et
[`RUNBOOK-FR.md`](./RUNBOOK-FR.md).

L'inventaire complet des options est aussi accessible via `--help` sur
chaque sous-commande :

```bash
perf-sentinel --help
perf-sentinel <subcommand> --help
```

Les sections ci-dessous ne sont pas exhaustives pour chaque
sous-commande. Elles se concentrent sur les surfaces utilisateur qui
bénéficient d'une explication en prose (workflow, valeurs par défaut,
codes de sortie). Pour la liste complète des flags, préférez `--help`.

## capture

Reçoit des traces OTLP et les écrit dans un fichier, pour qu'un job de
CI produise l'entrée sur laquelle `analyze --ci` pose sa gate sans faire
tourner d'OpenTelemetry Collector. Cette commande reçoit et écrit, elle
n'analyse jamais : le verdict reste à `analyze`, sur un fichier que vous
pouvez conserver comme artefact de build, rejouer avec d'autres seuils,
ou comparer à une référence avec `diff`.

L'application n'a besoin d'aucun réglage propre à Perf Sentinel, juste
des variables d'endpoint standard. Précisez aussi le protocole : les SDK
ne s'accordent pas sur leur défaut, et un endpoint pointé sur le mauvais
n'exporte rien sans le dire.

```bash
# OTLP HTTP, le défaut de la plupart des SDK
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318
export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf

# OTLP gRPC, le défaut d'opentelemetry-java
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
export OTEL_EXPORTER_OTLP_PROTOCOL=grpc
```

### Deux formes

**Envelopper l'étape de test**, la plus solide :

```bash
perf-sentinel capture --output traces.json -- mvn verify
perf-sentinel analyze --ci --input traces.json
```

Les ports sont liés avant que la commande démarre, aucun export ne se
perd donc dans une course au démarrage, et la capture s'arrête à la fin
de la commande plutôt que sur un délai deviné. La commande hérite de
stdout et stderr sans altération, et son code de sortie est propagé :
un échec de tests reste un échec de job.

La commande enveloppée est celle qui lance vos tests, quelle qu'elle
soit. `capture` démarre un processus, peu lui importe lequel, et rien
là-dedans n'est propre à Java :

```bash
perf-sentinel capture --output traces.json -- ./gradlew integrationTest
perf-sentinel capture --output traces.json -- pytest tests/integration
perf-sentinel capture --output traces.json -- npm run test:e2e
perf-sentinel capture --output traces.json -- go test ./...
perf-sentinel capture --output traces.json -- dotnet test
perf-sentinel capture --output traces.json -- ./scripts/run-integration-tests.sh
```

La commande est exécutée directement, sans passer par un shell : un pipe
ou une chaîne `&&` doit vivre dans un script que `capture` enveloppe
ensuite.

Ce qui change d'un langage à l'autre, c'est l'instrumentation qui fait
exporter l'application, décrite dans
[`INSTRUMENTATION-FR.md`](INSTRUMENTATION-FR.md). `capture` ne fait
qu'écouter.

**Tests dans des conteneurs.** Quand l'application testée tourne dans un
conteneur voisin plutôt que sur le runner, liez l'écoute sur toutes les
interfaces et pointez l'exporteur vers l'hôte :

```bash
perf-sentinel capture --output traces.json --listen-address 0.0.0.0 -- docker compose up --exit-code-from tests
# dans le conteneur : OTEL_EXPORTER_OTLP_ENDPOINT=http://host.docker.internal:4318
#                     OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
```

**Écouter à côté d'une étape de test existante**, quand votre pipeline
contrôle la commande de test et qu'elle ne peut pas être préfixée :

```bash
perf-sentinel capture --output traces.json &
CAPTURE=$!
./scripts/run-integration-tests.sh
kill -TERM $CAPTURE && wait $CAPTURE
```

> **Préfixez l'étape existante, n'en ajoutez jamais une seconde.**
> `capture -- mvn verify` lance les tests une fois. Une nouvelle étape
> de pipeline à côté de l'existante ferait tourner toute la suite
> d'intégration deux fois, pour rien.

### Sortie

Du NDJSON, une requête OTLP par ligne, la forme que produit l'exporteur
`file` du Collector. `analyze`, `report` et `diff` la détectent
automatiquement, sans flag. Les requêtes sont écrites telles que reçues,
sans conversion, le fichier décrit donc ce que l'application a envoyé.

Le répertoire de `--output` est créé s'il manque, de sorte que
`--output target/traces.json` fonctionne sur un workspace CI vierge où
l'outil de build n'a pas encore créé `target/`. En mode enveloppe, cet
outil de build n'a pas encore tourné.

La progression et le décompte final vont sur **stderr**, jamais sur
stdout : en mode enveloppe, ce flux appartient à la commande enveloppée.
Ce résumé est ce qui permet de distinguer "aucun anti-pattern trouvé" de
"rien n'a jamais été exporté", et un fichier de traces vide est rejeté
par `analyze` au lieu d'être présenté comme une gate au vert.

### Options utiles

| Flag                                        | Défaut          | Pourquoi le changer                                                                                    |
|---------------------------------------------|-----------------|--------------------------------------------------------------------------------------------------------|
| `--listen-address`                          | `127.0.0.1`     | `0.0.0.0` quand l'application tourne dans un autre conteneur du même job                               |
| `--listen-port-grpc` / `--listen-port-http` | `4317` / `4318` | un port est déjà pris sur l'agent                                                                      |
| `--max-file-size`                           | `512` (Mio)     | une grosse suite. Au-delà du plafond le fichier reste valide mais incomplet, et le run sort en `2`     |
| `--grace-ms`                                | `2000`          | combien de temps continuer à écouter après la fin de la commande, pour le dernier flush de l'exporteur |

### Codes de sortie

- `0` : capture terminée.
- le code de la commande enveloppée quand elle a échoué, ce signal étant
  le plus important. Une commande tuée par un signal renvoie
  `128 + signal`, comme le ferait un shell, jamais `0`.
- `1` : la capture elle-même a échoué (port pris, fichier non
  inscriptible). Les deux sont détectés avant le démarrage de la commande
  enveloppée, une capture qui ne peut pas écouter ne laisse donc jamais
  une suite de tests en cours.
- `2` : le fichier de traces est en deçà du run, soit parce que le
  plafond de taille a été atteint, soit parce que des requêtes n'ont pas
  pu être mises en file assez vite. Tout verdict qui en sortirait
  sous-estimerait le run.

## ack

Acquitter des findings via l'API daemon ack introduite en 0.5.20.
Trois sous-actions : `create`, `revoke`, `list`.

Le CLI consomme les endpoints HTTP du daemon
(`POST/DELETE /api/findings/{sig}/ack` et `GET /api/acks`). Il ne
modifie pas la baseline TOML CI
(`.perf-sentinel-acknowledgments.toml`). Ce fichier est fait pour être
édité à la main et livré via revue de PR. Voir
[`ACK-WORKFLOW-FR.md`](./ACK-WORKFLOW-FR.md) pour choisir entre les
deux mécanismes.

### Synopsis

```bash
perf-sentinel ack [OPTIONS] <SUBCOMMAND>
```

Options de niveau supérieur (s'appliquent aux trois sous-actions) :

- `--daemon <URL>` : endpoint HTTP du daemon. Par défaut
  `$PERF_SENTINEL_DAEMON_URL` puis `http://localhost:4318`.

### `ack create`

Créer un nouvel acquittement.

```bash
perf-sentinel ack create \
  --signature "n_plus_one_sql:order-svc:_api_orders:0123456789abcdef0123456789abcdef" \
  --reason "deferred to next sprint" \
  --expires 7d
```

Options :

- `--signature <SIG>` (ou `-s`) : signature du finding à acquitter. Si
  omis, le CLI lit la signature depuis stdin (uniquement quand stdin
  n'est pas un TTY). La lecture stdin est plafonnée à 1 KiB, un pipe
  `cat /dev/urandom` ne peut donc pas saturer la mémoire avant que le
  validateur côté daemon rejette l'entrée.
- `--reason <TEXT>` (ou `-r`) : requis, description libre de la
  raison de l'acquittement.
- `--expires <ISO8601_OR_DURATION>` : expiration de l'ack. Accepte un
  datetime ISO8601 (`2026-05-11T00:00:00Z`) ou une durée relative
  (`7d`, `24h`, `30m`). Omettre pour un ack permanent.
- `--by <NAME>` : identité de la personne qui acquitte. Se rabat sur
  `$USER`, puis `"anonymous"`.
- `--api-key-file <PATH>` : voir "Authentification" plus bas.

### `ack revoke`

Retirer un acquittement existant.

```bash
perf-sentinel ack revoke \
  --signature "n_plus_one_sql:order-svc:_api_orders:0123456789abcdef0123456789abcdef"
```

### `ack list`

Énumérer les acquittements daemon actifs.

```bash
perf-sentinel ack list
perf-sentinel ack list --output json
```

`ack list` ne montre que les acks daemon. Les acks TOML CI restent
visibles directement dans
`.perf-sentinel-acknowledgments.toml`. Le daemon plafonne la réponse
à 1000 entrées. Les colonnes `AT` et `EXPIRES_AT` sont en heure
locale, `--output json` reste en UTC.

### Authentification

Quand le daemon impose une clé API (`[daemon.ack] api_key` dans la
configuration du daemon), le CLI la résout dans cet ordre :

1. Variable d'environnement `PERF_SENTINEL_DAEMON_API_KEY`.
2. `--api-key-file <PATH>`. Le contenu du fichier est lu et tout
   saut de ligne final est supprimé.
3. Invite interactive `rpassword` (sans écho) si le daemon retourne
   401 et stdin est un TTY. La valeur collée est plafonnée à 1 KiB.

`query` prend le même `--api-key-file <PATH>` une fois, avant ou après
sa sous-commande, et le résout de la même façon (étapes 1 et 2, pas
d'invite). Chaque sous-commande `query` envoie la clé sur chaque
requête, donc un daemon avec `[daemon] require_read_key = true`, qui
refuse toute lecture sans clé, répond aussi à `findings`, `explain`,
`correlations` et `status`. Une lecture refusée sort en 1 et nomme le
401. La clé de lecture `[daemon] read_api_key` suffit pour chaque
lecture de `query`. `ack`, et l'acquittement depuis `query inspect`,
ont besoin de la clé d'ack. Chaque autre lecteur d'un tel daemon a
aussi besoin de la clé : PerfSentinelHub, les datasources Grafana,
`curl …/api/export/report | perf-sentinel report`, le rapport HTML live
et le plugin JetBrains. N'activez pas l'interrupteur sur un daemon que
lisent les IDE des développeurs tant que le plugin JetBrains installé
n'envoie pas de secret par endpoint : les versions antérieures n'en
envoient aucun et reçoivent `401` à chaque rafraîchissement. Voir
[QUERY-API-FR.md](QUERY-API-FR.md#porte-de-lecture).

```bash
perf-sentinel query --api-key-file ~/.config/perf-sentinel/read.key findings
perf-sentinel query explain --trace-id abc123 --api-key-file ~/.config/perf-sentinel/read.key
```

Il n'y a pas de flag `--api-key <SECRET>` direct, car passer des
secrets en ligne de commande les expose via la liste des processus et
l'historique du shell.

Sur Unix, `--api-key-file` est ouvert avec `O_NOFOLLOW` (les liens
symboliques sont refusés) et le CLI affiche un avertissement d'une
ligne sur stderr si le fichier est lisible par le groupe ou tous
(`mode & 0o077 != 0`). L'avertissement n'est émis que si stderr est
un TTY : dans les contextes CI / Docker / systemd où stderr n'est pas
un TTY, l'avertissement est supprimé pour garder les logs de build
propres. Les opérateurs dans ces environnements doivent fixer les
permissions du fichier de manière déclarative (k8s Secret avec
`defaultMode: 0o400`, `StatefulSet` monté depuis un `Secret`, etc.)
plutôt que de compter sur l'avertissement runtime.

### Résolution de l'URL du daemon

`--daemon <URL>` > variable `PERF_SENTINEL_DAEMON_URL` > défaut
`http://localhost:4318`. Le défaut correspond à `perf-sentinel watch`,
qui écoute sur le port standard OTLP/HTTP.

### Codes de sortie

- `0` : succès.
- `1` : erreur générique (échec réseau, erreur d'analyse syntaxique,
  signature absente sur stdin).
- `2` : erreur client (HTTP 4xx). Inclut 401 (non autorisé), 409
  (déjà acquitté), 404 (non acquitté sur revoke), 400 (format de
  signature invalide).
- `3` : erreur serveur (HTTP 5xx). Inclut 503 (store ack désactivé),
  500 (échec d'écriture) et 507 (store ack plein).

Les erreurs sont écrites sur stderr avec une cause sur une ligne et
une indication exploitable le cas échéant.

## Hub : login, logout, hub

`login`, `logout` et `hub` s'adressent à un
[Perf Sentinel Hub](https://github.com/Institut-du-Numerique-Responsable/PerfSentinelHub),
pas à un daemon. La ligne d'aide de chaque action `hub` commence par
« Through the Hub: », ce qui distingue `hub analyze`, `hub ack` et
`hub findings` de `analyze`, `ack` et `query findings`.

### `login` et `logout`

```bash
perf-sentinel login --hub https://hub.example.com
# Open https://hub.example.com/device in a browser and enter the code BCDF-GHJK (expires in 10 minutes).
# Signed in to https://hub.example.com until 2026-10-14 18:42:00 (local time).
```

`login` déroule le device flow du Hub ; à défaut de `--hub`, il lit
`PERF_SENTINEL_HUB_URL`. Il affiche l'adresse `/device` du
Hub lui-même et un code, et n'ouvre rien : vous tapez le code sur cette
page, dans un navigateur connecté au Hub, puis vous approuvez.
N'approuvez qu'un code issu d'un `login` que vous venez de lancer
vous-même. La CLI interroge le Hub à l'intervalle qu'il donne, 5 secondes
plus lentement à chaque réponse `slow_down`, jusqu'à ce que le code soit
approuvé, refusé ou expiré (code de sortie 1 dans les deux derniers cas).
Le jeton dure autant que la session navigateur qui l'a approuvé, sans
rafraîchissement : relancez `login` quand il expire.

Un Hub sans connexion terminal, parce que son contrôle d'accès est
désactivé, que `TrustedProxyNetworks` n'est pas renseigné ou qu'il est
antérieur à la fonction, répond à la première requête par une
redirection, un `401` ou un `404` : `login` affiche alors « This Hub does
not offer terminal sign-in » et sort en 1.

Le jeton est écrit dans `$XDG_CONFIG_HOME/perf-sentinel/hub-token.json`,
sinon `~/.config/perf-sentinel/hub-token.json`, ou
`%APPDATA%\perf-sentinel\hub-token.json` sous Windows, avec l'URL du Hub
et l'expiration. Un seul Hub à la fois : un `login` vers un autre Hub le
remplace. Sous Unix, le répertoire est créé en `0700` et le fichier en
`0600`, écrit à côté de la cible puis renommé par-dessus. Un fichier de
jeton qui est un lien symbolique, lisible par d'autres utilisateurs
(`mode & 0o077 != 0`, comme ssh refuse une clé privée) ou expiré est
refusé avant toute requête ; pour un fichier lisible par d'autres
utilisateurs, lancez `logout`, qui révoque quand même son jeton.

`logout` révoque le jeton auprès du Hub (`POST /auth/revoke`) et supprime
le fichier, quelle que soit la réponse du Hub. Quand le Hub est
injoignable, il prévient que le jeton reste valide jusqu'à son expiration.
Un fichier lisible par d'autres utilisateurs est révoqué lui aussi, avec un
avertissement. Un lien symbolique ou un fichier illisible comme JSON est
supprimé sans révocation, faute de jeton à envoyer, et `logout` sort en 1 :
un jeton qu'il contenait reste valide sur le Hub jusqu'à son expiration.

### `hub`

```bash
perf-sentinel hub findings --environment production --status active --severity critical
perf-sentinel hub incidents --source-id prod-eu
perf-sentinel hub incidents 0123456789abcdef0123456789abcdef
perf-sentinel hub ack --source-id prod-eu -s "<signature>" -r "reporté au prochain sprint" --expires 7d
perf-sentinel hub unack --source-id prod-eu -s "<signature>"
perf-sentinel hub analyze --source-id tempo --service order-svc --lookback 1h -o report.html
```

| Action | Appels | Options |
|---|---|---|
| `findings` | `GET /api/findings` | `--service`, `--finding-type`, `--severity`, `--status`, `--source-id`, `--environment`, `--from`/`--to` (RFC 3339 UTC comme `2026-10-21T00:00:00Z`, ou millisecondes epoch ; un `--from` postérieur à `--to` est refusé avant toute requête), `--offset`, `--include-acked true\|false` (le défaut du Hub s'il est omis), `--signature`, `--limit` (50), `--format text\|json` |
| `incidents [ID]` | `GET /api/incidents[/{id}]` | `--service`, `--namespace`, `--source-id`, `--environment`, `--offset`, `--limit` (50), `--format` |
| `ack` | `POST /api/sources/{source_id}/acks` | `--source-id`, `-s/--signature` (stdin s'il est omis), `-r/--reason`, `--expires`. Pas de `--by` : le Hub vous nomme |
| `unack` | `POST /api/sources/{source_id}/acks/revoke` | `--source-id`, `-s/--signature` |
| `analyze` | `POST /api/analyses`, `GET /api/analyses/{id}` toutes les 2 s, puis `GET /reports/{id}.html` | `--source-id`, `--service`, `--lookback`, `--from`/`--to` (RFC 3339 UTC ou millisecondes epoch, envoyés en `from_ms`/`to_ms`), `--trace-id`, `--max-traces`, `-o/--output` (par défaut `hub-report-<id>.html`) |

La sortie texte reprend les mises en page de `query findings` et
`query incidents`. Une liste d'incidents ne porte pas leurs findings :
chacun indique combien il en a figé, et `hub incidents <ID>` les affiche.
`--format json` imprime la réponse du Hub telle quelle. `analyze` sort en
1 avec l'`error_code` du run quand celui-ci échoue, est interrompu ou
expire.

Le justificatif, dans cet ordre :

1. `PERF_SENTINEL_HUB_TOKEN`, un jeton terminal ou machine. Il exige une
   URL de Hub via `--hub` ou `PERF_SENTINEL_HUB_URL`.
2. Le jeton enregistré par `login`, dont le Hub devient la valeur par
   défaut. Un `--hub` qui désigne un autre Hub est refusé (« The stored
   token was issued by ... »).
3. Aucun des deux : la requête part sans en-tête `Authorization` et le Hub
   décide. Un Hub sans contrôle d'accès répond à `GET /api/findings` de
   façon anonyme.

Un jeton ne circule qu'en `https`, ou en `http` simple vers `localhost`,
`127.0.0.1` ou `::1` ; toute autre URL `http` est refusée avant la
requête. Le jeton n'apparaît jamais sur la ligne de commande, dans une
ligne de log ou dans un message d'erreur. Aucune redirection n'est
suivie : un `3xx` signifie que le Hub veut une session navigateur, soit
parce que la route demande `perf-sentinel login`, soit parce que le
contrôle d'accès du Hub est désactivé et que cette route exige une
session. Un `401` affiche la commande `login` à lancer, un `403`, `404` ou
`409` le `detail` du Hub, et un `503` sans détail « Hub busy, retry ».
Toute erreur sort en 1.

## Autres sous-commandes

`perf-sentinel query incidents [--service <NAME>] [--namespace <NAME>]
[--offset N] [--limit N] [--format text|json] [--api-key-file <PATH>]`
liste les incidents que l'alerting a postés au daemon (0.20.0+,
`[daemon.incidents]`), du plus récent au plus ancien. Chaque incident a
un bloc d'en-tête (genre, service en `ns/service` quand l'alerte portait
un namespace, début et fin en heure locale, la fenêtre, un marqueur de
capture qui dit si le ring détenait encore toute la fenêtre, combien de
findings ne se sont déclenchés qu'après le redémarrage, le détail de
l'alerte et l'identifiant), puis ses findings dans la mise en page de
`query findings`. `--service` et `--namespace` restreignent la liste à
une valeur chacun. `--limit` vaut 50 par défaut et le daemon le plafonne
à 100. `--offset` pagine au-delà des plus récents. Un 401 (passez la
clé), un 503 (`[daemon.incidents] enabled = false`) et un 404 (daemon
antérieur à 0.20.0) sortent chacun en 1 avec leur cause nommée.
`query monitor --api-key-file <PATH>` remet la même clé à chaque
interrogation du moniteur, onglet Incidents compris, voir
[`INSPECT-FR.md`](./INSPECT-FR.md).

Pour l'instant, voir `perf-sentinel <subcommand> --help` pour la
liste complète des options de `analyze`, `watch`, `query`, `report`,
`diff`, `explain`, `inspect`, `pg-stat`, `mysql-stat`, `tempo`, `jaeger-query`,
`demo`, `bench` et `calibrate`. Les commandes elles-mêmes sont
stables. Leur documentation en prose est complétée au fur et à mesure.

Les commandes de chaîne d'approvisionnement disposent d'une documentation
dédiée ailleurs : `disclose`, `disclose-pdf`, `verify-hash` et `hash-bake` sont
couverts dans [`REPORTING-FR.md`](./REPORTING-FR.md), avec le
contexte signature et provenance dans
[`SUPPLY-CHAIN-FR.md`](./SUPPLY-CHAIN-FR.md). Les commandes des cartes
à partager, `card keygen`, `card render` et `verify-card`, sont couvertes
dans [`SHARE-CARD-FR.md`](./SHARE-CARD-FR.md), avec leurs lignes JSON,
leurs codes de sortie et leurs charges utiles. La complétion shell et
la page de manuel sont documentées plus bas sur cette page.

## Heures au terminal

Toute heure affichée pour une personne est en heure locale : la ligne
`Window:` de la sortie texte d'`analyze`, `report` et `diff`, les
panneaux d'`inspect`, et les sorties d'`ack`, de `query`, de `login`, de
`logout` et de `hub`. JSON,
SARIF, NDJSON, CSV et l'API du daemon restent en UTC.

Le fuseau vient de `TZ` quand elle est définie, sinon du système. Une
`TZ` qui nomme un fuseau absent de la base du système se résout sur
une base IANA embarquée dans le binaire au lieu de retomber en UTC.
C'est le cas dans l'image conteneur `FROM scratch`, qui n'embarque
aucune base de fuseaux : `docker run -e TZ=Europe/Paris ...` affiche
l'heure de Paris. Une règle POSIX comme `CET-1CEST,M3.5.0,M10.5.0/3`
fonctionne aussi.

## Complétion shell

`perf-sentinel completions <shell>` écrit un script de complétion sur
stdout. Shells pris en charge : `bash`, `zsh`, `fish`, `powershell`,
`elvish`. Rediriger la sortie vers le chemin de complétion du shell :

```bash
# Zsh (oh-my-zsh, prezto, fpath manuel)
perf-sentinel completions zsh > ~/.zfunc/_perf-sentinel

# Bash
perf-sentinel completions bash > /usr/local/etc/bash_completion.d/perf-sentinel

# Fish
perf-sentinel completions fish > ~/.config/fish/completions/perf-sentinel.fish
```

Recharger le shell, ou `source` le fichier, après l'installation.
Régénérer le script après chaque mise à jour de `perf-sentinel` pour
que la complétion reste alignée sur les nouveaux flags et sous-commandes.

## Page de manuel

`perf-sentinel man` écrit une page de manuel roff sur stdout. Elle rend
la page de premier niveau, qui liste les sous-commandes (comme `git.1`).
Rediriger la sortie vers le chemin des pages de manuel :

```bash
perf-sentinel man > /usr/local/share/man/man1/perf-sentinel.1
```

`man perf-sentinel` fonctionne ensuite. Pour prévisualiser sans
installer :

```bash
perf-sentinel man > /tmp/perf-sentinel.1 && man /tmp/perf-sentinel.1
```

Régénérer la page après chaque mise à jour de `perf-sentinel` pour
qu'elle reste alignée sur les nouveaux flags et sous-commandes.
