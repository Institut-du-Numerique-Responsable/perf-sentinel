# Cartes à partager

Une carte à partager imprime le CO2 qu'une équipe a évité, mesuré par Perf Sentinel, à côté d'un équivalent du quotidien comme des repas ou des kilomètres en train. Elle est signée : chacun peut vérifier que les chiffres sont ceux qu'a mesurés le Perf Sentinel Hub du signataire, et que personne ne les a modifiés ensuite.

PerfSentinelHub construit les chiffres de la carte à partir de ses propres données et appelle le moteur pour la dessiner et la signer. Le moteur ne lit jamais la base du Hub : il reçoit un `card.json`, applique toutes les règles de cette page et refuse une carte dont les chiffres annonceraient plus que ce qu'ils montrent. Cette page est le contrat entre les deux (C2 pour les commandes, C3 pour les charges utiles) et la référence sur laquelle repose la page de vérification `perf-sentinel.dev/verifier`.

## Ce que montre une carte

Une carte a l'un de ces trois angles :

| Angle | `angle` | Ce qu'elle dit |
|---|---|---|
| Histoire | `story` | Un finding, son ampleur avant sa correction, et depuis quand il a disparu. |
| Courbe | `curve` | La part du travail évitable, période par période, de 2 à 12 barres. |
| Collectif | `collective` | Combien de corrections la mesure a confirmées sur une période, sur combien de services, par famille. |

Chaque carte se rend en anglais ou en français (`lang`) et dans l'un de ces deux formats :

- `card.png`, une image unique de 1080 x 1350, le format à publier.
- `card.pdf`, un carrousel de quatre pages (l'accroche, pourquoi c'est important, les chiffres avec leur équivalence, la preuve avec la méthode), en PDF/A-3b.

Les deux portent la charge utile complète signée. Le QR code de l'image et de la dernière page du carrousel contient l'URL de vérification avec la charge utile compacte.

## Commandes (C2)

Chaque commande affiche une ligne JSON sur stdout, qui fait contrat. stderr porte un détail lisible et n'est jamais analysé.

### `card keygen`

```bash
perf-sentinel card keygen --out-dir <DIR>
```

Génère une clé Ed25519, l'écrit dans `<DIR>/<empreinte>.pk8` (PKCS#8 v2, mode 0600, jamais à travers un lien symbolique, jamais par-dessus un fichier existant) et affiche :

```json
{"fingerprint":"ps1:BjFPvCKhcizo9T39wJxfhA","public_key":"ZFvd2BCZpTwhC04R9ptRGgO7rGWHkq5gtL4Zb1oXdDc"}
```

`public_key` est la clé brute de 32 octets en base64url, la valeur à publier. `card keygen` n'existe que sous Unix.

### `card render`

```bash
perf-sentinel card render --input card.json --signing-key <empreinte>.pk8 --out-dir <DIR> --format png|pdf
```

Lit `card.json` (64 Kio au plus) et la clé (1 Kio au plus, un fichier régulier, jamais un lien symbolique, et sous Unix lisible par son seul propriétaire), signe les charges utiles complète et compacte, et écrit `<DIR>/card.png` ou `<DIR>/card.pdf`. Rien n'est écrit si la carte ne se rend pas. La même entrée et la même clé donnent les mêmes octets. La commande affiche :

```json
{"result":"rendered","files":["card.png"],"fingerprint":"ps1:...","compact_url":"https://perf-sentinel.dev/verifier#c=..."}
```

La clé privée ne passe jamais en argument de ligne de commande : le Hub passe le chemin d'un fichier de son répertoire temporaire privé.

### `verify-card`

```bash
perf-sentinel verify-card <card.png|card.pdf|URL> [--keys <published.json>] [--online [--doh <URL>]]
```

Lit un fichier de carte (16 Mio au plus, un PNG ou un PDF d'après ses premiers octets) ou une URL de vérification (toute cible qui commence par `https://` ou `http://`), vérifie la signature, puis le lien de la clé avec le domaine de la carte. Elle fonctionne hors ligne par défaut. `--keys` lit une copie locale du `/.well-known/perf-sentinel-key` du domaine (64 Kio au plus). `--online` lit les clés publiées par le réseau, voir [`--online`](#--online). `--keys` et `--online` s'excluent, et `--doh` demande `--online`. La commande affiche :

```json
{"integrity":"ok","level":2,"domain":"acme.fr","key":"current","reason":"key_published"}
```

| Clé | Valeurs |
|---|---|
| `integrity` | `ok`, ou `tampered` quand la signature ne tient pas sur la charge utile ou que la `key` de la charge utile n'est pas l'empreinte de sa clé publique. |
| `level` | `1` : la signature tient, rien ne lie la clé au domaine. `2` : le domaine publie cette clé pour le jour où la carte a été signée. |
| `domain` | Le `org.domain` de la carte, ou `null`. |
| `key` | `current`, `retired`, `revoked` ou `unknown`, ce que la liste publiée dit de la clé. |
| `reason` | Pourquoi, tiré de la liste fermée ci-dessous. |

La signature est vérifiée d'abord, sur les octets exacts que porte la carte. Seule une charge utile intacte est ensuite confrontée aux règles : une mauvaise signature donne `tampered` quoi que disent les octets, et une signature valide sur une charge utile qui enfreint une règle donne `malformed_input`.

### Codes de sortie et raisons

Une erreur affiche `{"result":"<famille>","reason":"<raison>"}`, plus `"field"` sur `unsupported_characters` quand le texte vient de `free.title`, `free.team`, `free.label` ou `org.name`. Tout autre texte que les polices ne savent pas dessiner (périmètre, service, endpoint) donne `unsupported_characters` sans `field`.

| Sortie | `result` | Raisons de `card keygen` et `card render` |
|---|---|---|
| 0 | | Fait. |
| 1 | `refused` | `period_not_calendar`, `period_not_closed`, `incomplete_period`, `volume_not_avoidable`, `threshold_not_applicable`, `unknown_equivalence`, `equivalence_out_of_scale`, `unsupported_language`. La carte annoncerait ce que ses chiffres ne montrent pas. |
| 3 | `input_error` | `missing_file`, `file_too_large`, `malformed_input`, `signing_key_permissions`, `signing_key_unreadable`, `key_exists`, `output_unwritable`. |
| 5 | `unrenderable` | `compact_too_large`, `unsupported_characters`, `unsupported_version`, `png_encode_failed`, `pdf_export_failed`. |

| Sortie | Ligne et raisons de `verify-card` |
|---|---|
| 0 | `integrity` `ok`. Niveau 2 : `key_published`, `signed_before_revocation`. Niveau 1 : `no_key_list`, `key_not_published`. |
| 1 | `integrity` `tampered` (`signature_invalid`, `fingerprint_mismatch`), ou une clé dont la liste ne se porte pas garante (`signed_after_revocation`, `signed_outside_validity`, `public_key_mismatch`), au niveau 1. |
| 3 | `{"result":"input_error"}` : `missing_file`, `file_too_large`, `malformed_input`, `no_card_payload`, `duplicate_card_payload`, `no_domain`. |
| 4 | `--online` n'a pas pu lire les clés du domaine : la ligne de vérification au niveau 1 avec `binding_check_failed`, ou `no_http_client` sur une compilation sans les features HTTP. |
| 5 | `{"result":"unrenderable","reason":"unsupported_version"}` : le format de la carte est plus récent que ce moteur. |

Une charge utile dont le dernier caractère base64url est remplacé par un caractère non canonique (bits de fin non nuls) donne `malformed_input`, sortie 3, et non `tampered` : les deux rejettent la carte. Une enveloppe de plus de 64 Kio dans un PDF donne `malformed_input`, un PDF de plus de 4 Mio `file_too_large`.

## Charges utiles (C3)

### `card.json`, l'entrée du rendu

Le Hub l'écrit, le moteur le lit. Les clés inconnues sont refusées, chaque nombre est un entier de 0 à 2^53 − 1 (9 007 199 254 740 991, le plus grand qu'un navigateur relit exactement), et les dates s'écrivent `YYYY-MM-DD`.

| Clé | Requise | Contenu et bornes |
|---|---|---|
| `angle` | oui | `story`, `curve` ou `collective`. |
| `lang` | oui | `en` ou `fr`. Toute autre valeur donne `malformed_input`. |
| `period` | oui | `{"kind","from","to"}`. `kind` vaut `week` (du lundi au dimanche), `month`, `quarter`, `semester` (de janvier à juin, de juillet à décembre) ou `year`. `from` et `to` bornent exactement une période civile de ce type (`period_not_calendar`), qui se termine avant le jour UTC de `generated_at_ms` (`period_not_closed`). |
| `scope` | oui | `{"environment","sources","covered_days","restart_days","sampled_permille"}`. `environment` est facultatif. `sources` n'est pas vide. `covered_days` vaut le nombre de jours de la période (`incomplete_period`), `restart_days` vaut au plus `covered_days`, `sampled_permille` va de 0 à 1000. `environment` et `sources`, joints par `", "`, tiennent en 120 caractères. |
| `figures` | oui | Des entiers dont le nom porte l'unité, lus selon l'angle, voir plus bas. |
| `story` | sur une histoire seulement | `{"finding_type","service","endpoint","observed_at","done_at"}`. `observed_at` est facultatif. `endpoint` tient en 200 caractères. `done_at` tombe dans la période ou avant, `observed_at` le jour de `done_at` ou avant. |
| `before_fix_period` | quand la carte en imprime une | `{"from","to"}`, une période civile de type `period.kind`, voir [la période d'avant la correction](#la-période-davant-la-correction). |
| `equivalence` | non | Un `id` de la table des équivalences. Absent, le moteur en choisit une. |
| `free` | oui, `{}` si vide | `{"title","team","label"}`, chacun facultatif : titre 48 caractères, équipe 48, libellé 80 (sur une histoire seulement). |
| `org` | oui | `{"name","domain"}`. `name` tient en 60 caractères. `domain` est facultatif, un nom DNS public : ASCII minuscule, 253 au plus, au moins deux étiquettes de 1 à 63 `[a-z0-9-]` sans tiret en bord, la dernière pas entièrement numérique. |
| `generated_at_ms` | oui | Le moment où le Hub a construit la carte, en millisecondes epoch. |

Chaque texte est déjà débarrassé de ses espaces de bord (une valeur qui diffère de sa version nettoyée est refusée, jamais nettoyée en silence), non vide, et ne contient aucun caractère de contrôle, bidirectionnel ou invisible.

`figures` selon l'angle :

| Angle | Clés |
|---|---|
| Histoire, types N+1 et redondant | `cited_ops`, `before_fix_ops` (facultatif, le volume de la période d'avant la correction), `co2_low_ug` (facultatif, seulement avec `before_fix_ops`). Pas de seuil (`threshold_not_applicable`). |
| Histoire, types `slow_*` | `cited_ms`, `threshold_ms` (facultatif). |
| Histoire, `excessive_fanout`, `chatty_service`, `pool_saturation`, `serialized_calls` | `cited_ops`, `threshold_ops` (facultatif). |
| Courbe | `waste_permille` (2 à 12 valeurs d'au plus 1000, la plus ancienne d'abord, la période de la carte en dernier), `avoidable_ops` et `co2_low_ug` (facultatif) pour la période de la carte. |
| Collectif | `confirmed_fixes_count` (au moins 1), `previous_fixes_count` (facultatif, absent quand le Hub ne peut pas compter honnêtement la période précédente), `services_count` (de 1 à `confirmed_fixes_count`), `fixes_by_family_count` (`extra_calls`, `slow_calls`, `call_architecture`, dont la somme vaut `confirmed_fixes_count`), `before_fix_ops` (facultatif, demande une correction `extra_calls`), `co2_low_ug` (facultatif, seulement avec `before_fix_ops`). |

Seuls les findings N+1 et redondants ont un volume évitable : `before_fix_ops` sur tout autre type d'histoire donne `volume_not_avoidable`. Sur un collectif, `before_fix_ops` additionne le volume d'avant les seules corrections N+1 et redondantes, et la carte nomme ce sous-ensemble.

Les trois fixtures de `tests/fixtures/card/` sont des exemples complets : `story.json` (un N+1 avec son volume et son CO2), `curve.json` (trois mois) et `collective.json` (sept corrections, dont quatre N+1 ou redondantes).

```json
{
  "angle": "story",
  "lang": "en",
  "period": { "kind": "month", "from": "2026-09-01", "to": "2026-09-30" },
  "scope": { "environment": "production", "sources": ["prod-eu"], "covered_days": 30, "restart_days": 0, "sampled_permille": 1000 },
  "figures": { "cited_ops": 41, "before_fix_ops": 1937000, "co2_low_ug": 12400000000 },
  "story": { "finding_type": "n_plus_one_sql", "service": "shop", "endpoint": "GET /cart/{id}", "observed_at": "2026-08-20", "done_at": "2026-08-28" },
  "before_fix_period": { "from": "2026-07-01", "to": "2026-07-31" },
  "free": {},
  "org": { "name": "Acme", "domain": "acme.fr" },
  "generated_at_ms": 1790856000000
}
```

### La période d'avant la correction

La carte nomme la période d'où viennent ses chiffres « avant la correction », et les deux charges utiles la signent. Elle est présente exactement quand la carte en imprime une, et doit être :

- sur une histoire avec `before_fix_ops` : la période civile qui précède celle qui contient `done_at`,
- sur une histoire avec un seuil : la période qui contient `observed_at`, alors requis (la trace d'où vient son chiffre),
- sur un collectif avec `before_fix_ops` : la période qui précède celle de la carte.

Tout autre cas donne `malformed_input`.

### La charge utile complète

La charge utile complète est celle que portent le PNG et le PDF. Elle reprend `card.json` avec trois changements : `v` (1), l'objet `equivalence` résolu, et la clé.

| Clé | Contenu |
|---|---|
| `v` | La version du format, `1`. Une valeur plus haute donne `unsupported_version`. |
| `angle`, `lang`, `period`, `scope`, `figures`, `story`, `before_fix_period`, `free`, `org`, `generated_at_ms` | Comme dans `card.json`. |
| `equivalence` | `{"id","per_unit_ug","unit","source","vintage"}`, absent quand la carte n'imprime pas d'équivalence. |
| `key` | L'empreinte de la clé de signature. |
| `public_key` | La clé publique Ed25519 brute de 32 octets, en base64url. |

Le vérificateur applique aussi les règles de `card.json` à une charge utile signée, puisque n'importe qui peut fabriquer une clé. L'équivalence est vérifiée pour elle-même (chaînes non vides, facteur supérieur à 0, nombre d'au moins 2 sur le CO2 imprimé), jamais contre la table embarquée, pour qu'une carte reste vérifiable après une mise à jour de la Base Carbone.

La fixture d'histoire se signe ainsi :

```json
{"angle":"story","before_fix_period":{"from":"2026-07-01","to":"2026-07-31"},"equivalence":{"id":"meal","per_unit_ug":2040000000,"source":"ADEME, Base Carbone","unit":"meal","vintage":"V23.6 (2025-07-03)"},"figures":{"before_fix_ops":1937000,"cited_ops":41,"co2_low_ug":12400000000},"free":{},"generated_at_ms":1790856000000,"key":"ps1:BjFPvCKhcizo9T39wJxfhA","lang":"en","org":{"domain":"acme.fr","name":"Acme"},"period":{"from":"2026-09-01","kind":"month","to":"2026-09-30"},"public_key":"ZFvd2BCZpTwhC04R9ptRGgO7rGWHkq5gtL4Zb1oXdDc","scope":{"covered_days":30,"environment":"production","restart_days":0,"sampled_permille":1000,"sources":["prod-eu"]},"story":{"done_at":"2026-08-28","endpoint":"GET /cart/{id}","finding_type":"n_plus_one_sql","observed_at":"2026-08-20","service":"shop"},"v":1}
```

### La charge utile compacte

La charge utile compacte est celle que porte le QR code : l'angle, la période, les chiffres que la carte imprime en gros, la période d'avant la correction, le domaine, la clé publique et le jour. Les jours sont des jours epoch, comptés depuis le 1970-01-01 UTC.

| Clé | Contenu |
|---|---|
| `v` | `1`. |
| `a` | L'angle : `s` histoire, `c` courbe, `k` collectif. |
| `p` | `[type, from, to]` : la lettre du type, puis le premier et le dernier jour epoch de la période. Types : `w` semaine, `m` mois, `q` trimestre, `h` semestre, `y` année. |
| `f` | Les chiffres, voir plus bas. |
| `r` | `[from, to]`, les jours epoch de la période d'avant la correction, absent quand la carte n'en imprime pas. Jamais sur une courbe. |
| `d` | `org.domain`, absent quand la carte n'en nomme pas. |
| `k` | La clé publique en base64url, comme `public_key`. |
| `g` | Le jour epoch de `generated_at_ms`, le jour où la carte a été signée. |

| Angle | Clés de `f` |
|---|---|
| Histoire | `o` `cited_ops`, `m` `cited_ms`, `b` `before_fix_ops`, `t` `threshold_ops`, `u` `threshold_ms`. L'un des ensembles `o`, `b o`, `o t`, `m`, `m u`. |
| Courbe | `w` le `waste_permille` de la période de la carte, `x` celui de la période qui la précède. |
| Collectif | `n` `confirmed_fixes_count`, `e` `services_count`. |

La charge utile compacte de la fixture d'histoire :

```json
{"a":"s","d":"acme.fr","f":{"b":1937000,"o":41},"g":20727,"k":"ZFvd2BCZpTwhC04R9ptRGgO7rGWHkq5gtL4Zb1oXdDc","p":["m",20697,20726],"r":[20635,20665],"v":1}
```

### Enveloppes

- **Enveloppe de fichier** : `{"payload":"<base64url de la charge utile complète canonique>","signature":"<base64url de la signature Ed25519 de 64 octets>"}`, sans autre clé. Le PNG la porte dans un chunk `iTXt` au mot-clé `perf-sentinel-card`, avant les données d'image. Le PDF la porte en pièce jointe `perf-sentinel-card.json`. L'enveloppe tient en 64 Kio. Un second chunk `perf-sentinel-card` avant les données d'image, ou une seconde pièce jointe de ce nom, donne `duplicate_card_payload`.
- **URL compacte** : `https://perf-sentinel.dev/verifier#c=<base64url de la charge utile compacte canonique>.<base64url de sa signature>`. Le jeton voyage dans le fragment, qu'un navigateur n'envoie jamais au serveur. Un lecteur prend ce qui suit le premier `#c=` et le coupe sur son dernier `.`.

### Règles d'encodage

- **Octets canoniques.** Une charge utile se signe en JSON, clés triées à chaque niveau, sans espaces, chaînes en UTF-8 telles qu'écrites (seuls `"`, `\` et les caractères de contrôle sont échappés, et les textes n'en contiennent aucun). Un vérificateur vérifie la signature sur les octets que porte la carte et ne les resérialise jamais.
- **base64url strict.** L'alphabet `A-Z a-z 0-9 - _`, sans remplissage, une longueur qui ne vaut pas 1 modulo 4, et des bits de fin à zéro, pour qu'un seul caractère modifié ne décode jamais vers les mêmes octets.
- **Empreinte.** `ps1:` puis le base64url des 16 premiers octets du SHA-256 de la clé publique brute de 32 octets : 26 caractères.
- **Entiers seulement.** Chaque nombre est un entier de 0 à 2^53 − 1, l'unité dans le nom de la clé (`_ug`, `_ops`, `_ms`, `_permille`, `_count`, `_days`). Un flottant, un nombre négatif ou un entier plus grand donne `malformed_input`.
- **Signature.** Ed25519 (RFC 8032) sur les octets canoniques. Elle est déterministe : une charge utile et une clé donnent une seule signature.

### Résolution de C3

L'addendum du 2026-10-30 laissait quelques points ouverts, que ce moteur tranche :

- Les champs de l'histoire sont sous une clé `story` de premier niveau, à côté de `figures`, pas dedans.
- `lang` (`en` ou `fr`) est signé dans la charge utile complète.
- `org.domain` est facultatif. Une carte qui n'en a pas ne peut atteindre que le niveau 1.
- `before_fix_period` est signé dans la charge utile complète, et en `r` dans la compacte (point 8 de l'addendum), pour que la page de vérification puisse nommer le mois d'avant la correction.
- Une courbe compare la période de la carte avec celle qui la précède (`x`).
- Un fichier porte l'enveloppe JSON, l'URL porte le jeton `<charge>.<signature>`.

## Publication des clés

Une organisation lie ses clés à son domaine à deux endroits, l'un ou l'autre ou les deux. Le Hub affiche les deux formes, prêtes à copier.

**DNS.** Un enregistrement TXT par clé sous `_perf-sentinel.<domaine>` :

```text
_perf-sentinel.acme.fr. TXT "v=ps1; k=ps1:BjFPvCKhcizo9T39wJxfhA; from=2026-09-01"
```

L'enregistrement s'écrit `v=ps1; k=<empreinte>; from=YYYY-MM-DD[; to=YYYY-MM-DD][; revoked=YYYY-MM-DD]`. Il doit se lire exactement : un enregistrement dont le premier champ n'est pas `v=ps1`, avec un champ sans `=`, une date invalide ou un paramètre donné deux fois est ignoré, une faute de frappe se lit donc comme « non publiée ». Les paramètres inconnus sont ignorés. Un enregistrement coupé en plusieurs chaînes entre guillemets est recollé.

**Well-known.** `https://<domaine>/.well-known/perf-sentinel-key` :

```json
{"keys":[
  {"fingerprint":"ps1:BjFPvCKhcizo9T39wJxfhA","public_key":"ZFvd2BCZpTwhC04R9ptRGgO7rGWHkq5gtL4Zb1oXdDc","from":"2026-09-01","to":null,"revoked":null}
]}
```

`public_key`, `to` et `revoked` peuvent être absents ou `null`. Les clés inconnues et une empreinte listée deux fois sont refusées (`malformed_input`) : une liste nomme chaque clé une seule fois. `--keys` lit le même JSON depuis un fichier.

**Rotation et révocation.** Une clé est valide de `from` à `to`, bornes comprises, comparées au jour UTC où la carte a été signée (`generated_at_ms`, ou `g`).

| La liste dit | Carte signée | Résultat |
|---|---|---|
| Rien (aucune liste lue) | | Niveau 1, `unknown`, `no_key_list` |
| Pas cette empreinte | | Niveau 1, `unknown`, `key_not_published` |
| Une autre `public_key` pour cette empreinte | | Niveau 1, `unknown`, `public_key_mismatch`, sortie 1 |
| `revoked` | ce jour-là ou après | Niveau 1, `revoked`, `signed_after_revocation`, sortie 1 |
| Valide, sans `to` | avant `from` | Niveau 1, `current`, `signed_outside_validity`, sortie 1 |
| Valide, avec `to` | hors de `from`..`to` | Niveau 1, `retired`, `signed_outside_validity`, sortie 1 |
| `revoked` | avant ce jour, dans sa validité | Niveau 2, `revoked`, `signed_before_revocation` |
| Valide, sans `to` | dans sa validité | Niveau 2, `current`, `key_published` |
| Valide, avec `to` | dans sa validité | Niveau 2, `retired`, `key_published` |

Pour changer de clé, publiez la nouvelle avec son `from` et posez `to` sur l'ancienne : les cartes signées avec l'ancienne restent au niveau 2, `retired`. Pour révoquer une clé qui a fuité, posez `revoked` au jour de la fuite : toute carte signée ce jour-là ou après échoue, les plus anciennes restent au niveau 2 avec `revoked`. Quand l'enregistrement DNS et le fichier well-known listent tous deux une clé, chaque limite posée par l'un ou l'autre s'applique : `revoked` l'emporte sur `to`, et `to` sur l'absence de limite.

## `--online`

`verify-card --online` lit le domaine de la carte (`org.domain`, ou `d`), une carte sans domaine donne donc `no_domain`. La commande interroge deux endroits en même temps, par le client HTTP partagé du moteur :

1. Les enregistrements TXT de `_perf-sentinel.<domaine>`, auprès d'un service DNS-over-HTTPS JSON (`application/dns-json`) : `https://cloudflare-dns.com/dns-query` par défaut, ou l'URL `https://` que nomme `--doh`.
2. `https://<domaine>/.well-known/perf-sentinel-key`, sur le domaine lui-même, sans port ni partie utilisateur.

Chaque requête dispose de 5 s et lit 64 Kio au plus. Aucune redirection n'est suivie : un domaine dont l'apex redirige vers `www` fait échouer la lecture du well-known, publiez donc le fichier sur l'apex, ou comptez sur l'enregistrement DNS. Un 404, un NXDOMAIN ou l'absence d'enregistrement `v=ps1` signifient « ne publie rien ». Un délai dépassé, un statut d'erreur, une redirection ou un corps qui n'a pas la forme attendue signifient « vérification impossible ». Les deux réponses se fusionnent par empreinte. Quand une requête n'a pas pu vérifier et que l'autre n'a trouvé aucune clé, le résultat est `binding_check_failed` au niveau 1, sortie 4 : « vérification impossible » ne se lit jamais « non publiée », puisque la réponse manquante a pu révoquer la clé.

Ce sont les seuls appels réseau que le moteur fait pour une carte, et seulement avec `--online`. Sans lui, `verify-card` reste hors ligne.

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

## Textes de la carte

Chaque phrase qu'imprime une carte est listée ici, les françaises sur cette page et les anglaises sur la page anglaise. Le test `docs_list_every_story_sentence` échoue quand une phrase d'histoire change dans `crates/sentinel-core/src/report/card/copy.rs` et pas ici.

Une phrase d'histoire a deux formes. Sans seuil, `{lead}` est le libellé que l'utilisateur a donné à l'action suivi de ` : `, ou `Pour une action de l'application, `. Avec un seuil, `{before}` est la période d'avant la correction (« En août 2026 »), et le libellé, s'il y en a un, ouvre la phrase. `{n}` est le chiffre que cite l'histoire et `{threshold}` le seuil d'alerte.

| Types de finding | Sans seuil | Avec un seuil |
|---|---|---|
| `n_plus_one_sql`, `n_plus_one_http`, `n_plus_one_messaging` | `{lead}l'application allait chercher les informations {n} fois au lieu d'une.` | Aucune, ces types n'ont pas de seuil. |
| `redundant_sql`, `redundant_http` | `{lead}l'application demandait {n} fois la même information.` | Aucune. |
| `slow_sql`, `slow_http`, `slow_messaging` | `{lead}les passages les plus lents d'une étape prenaient {n} ou plus.` | `{before}, avant la correction, les passages les plus lents d'une étape prenaient {n} ou plus. Elle tourne toujours et ne déclenche plus l'alerte de lenteur, réglée à {threshold}.` |
| `excessive_fanout` | `{lead}une seule étape lançait {n} opérations.` | `{before}, avant la correction, une seule étape lançait {n} opérations. Elle tourne toujours et ne déclenche plus l'alerte, réglée à {threshold} opérations.` |
| `chatty_service` | `{lead}l'application sollicitait d'autres parties du système {n} fois.` | `{before}, avant la correction, l'application sollicitait d'autres parties du système {n} fois. L'action tourne toujours et ne déclenche plus l'alerte, réglée à {threshold} fois.` |
| `pool_saturation` | `{lead}{n} opérations tournaient au même moment, au plus fort.` | `{before}, avant la correction, {n} opérations tournaient au même moment, au plus fort. L'action tourne toujours et ne déclenche plus l'alerte, réglée à {threshold} opérations.` |
| `serialized_calls` | `{lead}l'application enchaînait {n} étapes l'une après l'autre.` | `{before}, avant la correction, l'application enchaînait {n} étapes l'une après l'autre. L'action tourne toujours et ne déclenche plus l'alerte, réglée à {threshold} étapes.` |

Le Hub n'envoie un seuil, et n'obtient donc la forme avec seuil, que s'il a vu des passages sous le seuil et aucune détection un jour après la correction (contrat C1). Sinon la carte prend la forme sans seuil et dit depuis quand le finding n'est plus détecté.

Les autres lignes :

| Où | Ligne |
|---|---|
| Histoire, sous la phrase | `Observé le {date}.` (quand `observed_at` est renseigné), puis `N'est plus détecté depuis le {date}.` (sans seuil) |
| Ligne CO2 d'une histoire | `{before}, avant la correction : {n} opérations inutiles, au moins {co2} CO2e (estimation)` |
| Ligne CO2 d'une courbe | `Évitable {période} : {n} opérations, au moins {co2} CO2e (estimation)` |
| Ligne CO2 d'un collectif | `{before}, avant la correction de travail fait plusieurs fois : ...` avec une correction N+1 ou redondante, `{before}, avant les {n} corrections de travail fait plusieurs fois : ...` avec plusieurs |
| Équivalence | `L'équivalent d'au moins {n} {unité}`, puis `1 {unité} = {facteur} CO2e. Source : {source}, {millésime}` |
| Collectif | `{n} corrections confirmées par la mesure, sur {s} services,` puis `contre {p} le mois précédent` (ou la semaine précédente, le trimestre précédent, le semestre précédent, l'année précédente) quand le Hub a compté la période précédente |
| Familles du collectif | `Ce qui a été corrigé` : `Travail fait plusieurs fois`, `Étapes lentes`, `Ordre dans lequel le travail était fait` |
| Courbe | `Part du travail évitable`, `en {période}, contre {x} en {période précédente}`, `Plus c'est bas, mieux c'est.`, et la légende `Part du travail évitable, mois par mois` (ou semaine par semaine, trimestre par trimestre, semestre par semestre, année par année) |
| Barres d'une étape lente | `Passages les plus lents, avant la correction` et, en pointillés, `Seuil d'alerte` |
| Preuve | `Correction confirmée par la mesure` (histoire) ou `Corrections confirmées par la mesure` (collectif), rien sur une courbe, qui ne nomme aucune correction |
| Périmètre | `Mesuré sur {sources}, {environnement}, {jours} jours`, puis `, {n} jours avec redémarrage` et `, sur {p} du trafic` quand ils s'appliquent |
| Organisation | `Organisation déclarée : {nom}` |
| Pied | `Généré le {date}`, `Mesuré avec Perf Sentinel, logiciel libre porté par l'Institut du Numérique Responsable` |
| Vérification | `Vérifiez la carte et lisez la méthode : perf-sentinel.dev/verifier` |
| Carrousel, page 2 | `Chaque opération inutile coûte du temps de serveur, de l'énergie et parfois de l'attente.` et `Les mesurer montre où l'application peut faire le même travail avec moins.` |
| Carrousel, page 4 | `Vérifiez ces chiffres` et `Méthode :` suivi du lien vers cette page |

En français, une espace insécable sépare un chiffre de son unité, les groupes de milliers, et précède `:` et `%`. Chaque chiffre est arrondi vers le bas : les nombres sont exacts sous 10 000, puis gardent deux chiffres significatifs (« 1,9 million »), les durées sont en millisecondes entières sous 100 ms, puis en secondes avec une décimale, et une part sous 0,1 % se lit « moins de 0,1 % ». Les unités que compte une équivalence sont `repas moyen(s)`, `km en TGV`, `km parcourus par une voiture moyenne` et `smartphone(s)`.

La ligne CO2 d'une histoire couvre la seule période d'avant la correction, celle d'une courbe la période de la carte, et celle d'un collectif la période d'avant la correction de ses seules corrections N+1 et redondantes. La ligne de périmètre d'une courbe compte les jours de toutes les périodes qu'elle montre, de la première barre à la dernière.

## QR code

Le QR code est en version 12 au plus, correction L : la carte vit sur un écran, et l'URL compacte ne tient pas en version 12 au niveau M. La version 12-L contient 367 octets. Le préfixe et la signature prennent 124 caractères de l'URL, ce qui laisse environ 182 octets au JSON compact. Avec les chiffres des fixtures, le plus long domaine qui tient fait 35 caractères sur une histoire avec `r`, 42 sur un collectif et 58 sur une courbe. Des volumes à dix chiffres ramènent l'histoire à 28 caractères. Un domaine plus long donne `compact_too_large`, jamais tronqué : le Hub doit donc vérifier la longueur avant de proposer le domaine.

## Lien vers la méthode

La dernière page du carrousel imprime `Méthode :` et l'adresse de cette page sur GitHub (`.../docs/FR/SHARE-CARD-FR.md`, ou `.../docs/SHARE-CARD.md` sur une carte en anglais), coupée seulement après un `/`. L'image imprime la page de vérification, `perf-sentinel.dev/verifier`, qui renvoie ici.

Les limites de ce que prouve une carte sont listées dans [`LIMITATIONS-FR.md`](./LIMITATIONS-FR.md#cartes-à-partager).
