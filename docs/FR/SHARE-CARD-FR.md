# Cartes à partager

Une carte à partager imprime ce que Perf Sentinel a mesuré d'un motif de gaspillage, comme son volume avant une correction et le CO2 chiffré sur ce volume, à côté d'un équivalent du quotidien comme des repas ou des kilomètres en train. Elle est signée : chacun peut vérifier qu'une clé a signé ces chiffres et que personne ne les a modifiés ensuite. Le niveau 2 de la vérification relie cette clé au domaine du signataire.

PerfSentinelHub construit les chiffres de la carte à partir de ses propres données et appelle le moteur pour la dessiner et la signer. Le moteur ne lit jamais la base du Hub : il reçoit un `card.json`, applique toutes les règles de cette page et refuse une carte dont les chiffres annonceraient plus que ce qu'ils montrent. Cette page est le contrat entre les deux (C2 pour les commandes, C3 pour les charges utiles) et la référence sur laquelle repose la page de vérification `perf-sentinel.dev/verifier`.

## Ce que montre une carte

Une carte a l'un de ces trois angles :

| Angle | `angle` | Ce qu'elle dit |
|---|---|---|
| Histoire | `story` | Un finding, son ampleur avant sa correction, et depuis quand il a disparu. |
| Courbe | `curve` | La part du travail évitable, période par période, de 2 à 12 barres. |
| Collectif | `collective` | Combien de corrections la mesure a confirmées sur une période, sur combien de services, par famille. |

Chaque carte se rend en anglais ou en français (`lang`) et dans l'un de ces quatre formats :

- `card.png`, une image unique de 1080 x 1350, le format à publier.
- `card.svg`, la même image en SVG, son texte dessiné en contours (voir [Le SVG](#le-svg)).
- `card.jxl`, les mêmes pixels en JPEG XL sans perte, un export d'archive que LinkedIn n'accepte pas (voir [Le JPEG XL](#le-jpeg-xl)).
- `card.pdf`, un carrousel de quatre pages (l'accroche, pourquoi c'est important, les chiffres avec leur équivalence, la preuve avec la méthode), en PDF/A-3b.

Les quatre portent la charge utile complète signée. Le QR code de l'image et de la dernière page du carrousel contient l'URL de vérification avec la charge utile compacte.

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
perf-sentinel card render --input card.json --signing-key <empreinte>.pk8 --out-dir <DIR> --format png|pdf|svg|jxl
```

Lit `card.json` (64 Kio au plus) et la clé (1 Kio au plus, un fichier régulier, jamais un lien symbolique, et sous Unix lisible par son seul propriétaire), signe les charges utiles complète et compacte, et écrit `<DIR>/card.png`, `<DIR>/card.pdf`, `<DIR>/card.svg` ou `<DIR>/card.jxl`. Rien n'est écrit si la carte ne se rend pas. La même entrée et la même clé donnent les mêmes octets. La commande affiche :

```json
{"result":"rendered","files":["card.png"],"fingerprint":"ps1:...","compact_url":"https://perf-sentinel.dev/verifier#c=..."}
```

La clé privée ne passe jamais en argument de ligne de commande : le Hub passe le chemin d'un fichier de son répertoire temporaire privé.

### `verify-card`

```bash
perf-sentinel verify-card <card.png|card.pdf|card.svg|card.jxl|URL> [--keys <published.json>] [--online [--doh <URL>]]
```

Lit un fichier de carte (16 Mio au plus, un PNG, un PDF, un JPEG XL ou un SVG d'après ses premiers octets, un JPEG XL commençant par sa boîte de signature ou par `FF 0A`, un SVG commençant par `<?xml` ou `<svg` après un BOM UTF-8 facultatif et des blancs) ou une URL de vérification (toute cible qui commence par `https://` ou `http://`), vérifie la signature de la charge utile et, sur un fichier, la signature du fichier lui-même, puis le lien de la clé avec le domaine de la carte. Elle fonctionne hors ligne par défaut. `--keys` lit une copie locale du `/.well-known/perf-sentinel-key` du domaine (64 Kio au plus). `--online` lit les clés publiées par le réseau, voir [`--online`](#--online). `--keys` et `--online` s'excluent, et `--doh` demande `--online`. La commande affiche :

```json
{"integrity":"ok","content":"intact","level":2,"domain":"acme.fr","key":"current","reason":"key_published","figures":{"angle":"story","period":{"kind":"month","from":"2026-09-01","to":"2026-09-30"},"before_fix_period":{"from":"2026-07-01","to":"2026-07-31"},"before_fix_ops":1937000,"cited_ops":41}}
```

Le même fichier avec un pixel changé affiche, avec la sortie 1 :

```json
{"integrity":"ok","content":"modified","level":1,"domain":null,"key":"unknown","reason":"content_modified","figures":null}
```

| Clé | Valeurs |
|---|---|
| `integrity` | `ok` quand la signature tient sur la charge utile et que la `key` de la charge utile est l'empreinte de sa clé publique, sinon `tampered`. `integrity` seul n'est pas le verdict : `"integrity":"ok"` dit seulement que la signature de la charge utile tient, et un fichier retouché l'affiche aussi, à côté de `"content":"modified"` et de la sortie 1. |
| `content` | Sur un fichier, `intact` quand c'est le fichier signé, `modified` sinon (voir [Ce que couvre la signature du fichier](#ce-que-couvre-la-signature-du-fichier)). `not_checked` sur une URL de vérification, qui n'a pas de fichier, et sur une ligne `tampered`. |
| `level` | `1` : la signature tient, rien ne lie la clé au domaine. `2` : le domaine publie cette clé pour le jour où la carte a été signée. |
| `domain` | Le `org.domain` de la carte, ou `null`. Aussi `null` quand la vérification s'est arrêtée avant les règles de la charge utile (`tampered`, `content_modified`). |
| `key` | `current`, `retired`, `revoked` ou `unknown`, ce que la liste publiée dit de la clé. |
| `reason` | Pourquoi, tiré de la liste fermée ci-dessous. |
| `figures` | Les grands chiffres de la carte tels que signés, pour qu'un lecteur les compare avec l'image, ou `null` quand la vérification s'est arrêtée avant les règles de la charge utile. |

`figures` contient `angle`, `period` (`{"kind","from","to"}`) et `before_fix_period` (`{"from","to"}`, absent quand la carte n'en imprime pas), puis les chiffres de la charge utile compacte sous ces noms. Les valeurs sont les entiers signés, sans arrondi : la page de vérification et le Hub les mettent en forme. Un fichier et son QR code donnent les mêmes `figures`.

| Nom | Lettre compacte | Unité |
|---|---|---|
| `cited_ops` | `o` | Opérations de la trace citée (histoire) |
| `cited_ms` | `m` | Millisecondes de l'appel cité (histoire) |
| `before_fix_ops` | `b` | Opérations sur la période avant correction (histoire) |
| `threshold_ops` | `t` | Opérations, le seuil que dépasse la trace citée (histoire) |
| `threshold_ms` | `u` | Millisecondes, le seuil que dépasse l'appel cité (histoire) |
| `last_waste_permille` | `w` | Pour mille d'I/O gaspillées sur la période de la carte (courbe) |
| `previous_waste_permille` | `x` | Pour mille d'I/O gaspillées sur la période précédente (courbe) |
| `confirmed_fixes_count` | `n` | Corrections confirmées par la mesure (collectif) |
| `services_count` | `e` | Services que ces corrections touchent (collectif) |

Les vérifications suivent un ordre fixe, et la première qui échoue décide de la ligne : l'enveloppe et son support, la signature de la charge utile sur les octets exacts que porte la carte, l'empreinte, la signature du contenu du fichier, les règles de la charge utile, puis le lien de la clé. Une mauvaise signature donne `tampered` quoi que disent les octets, et une signature valide sur une charge utile qui enfreint une règle donne `malformed_input`. Un fichier dont la signature de contenu ne tient pas donne `content_modified`, et sa clé n'est pas jugée : aucune lecture de `--keys`, aucune requête réseau sous `--online`.

Sur un fichier original, `content` dit si le fichier est celui qui a été signé, si bien qu'une vraie enveloppe copiée sur une image retouchée donne `content_modified`. Une image republiée ne garde que son QR code (LinkedIn réencode les images et retire leurs métadonnées), et le QR code prouve les chiffres, jamais les pixels qui l'entourent : comparez `figures`, ou la page de vérification, avec l'image.

#### Vérifier dans un script

Lisez d'abord le code de sortie, 0 seulement quand rien ne cloche, puis `reason` et `content` pour savoir pourquoi. Ne décidez jamais sur `integrity` seul : un fichier retouché affiche `"integrity":"ok"`.

```bash
perf-sentinel verify-card card.png > verdict.json; echo "exit $?"
jq -r .reason verdict.json
```

### Codes de sortie et raisons

Une erreur affiche `{"result":"<famille>","reason":"<raison>"}`, plus `"field"` sur `unsupported_characters` quand le texte vient de `free.title`, `free.team`, `free.label` ou `org.name`, et sur le `malformed_input` d'un `free.title` qui contient le marqueur de l'emplacement PDF (`"field":"free.title"`, voir C3). Tout autre texte que les polices ne savent pas dessiner (périmètre, service, endpoint) donne `unsupported_characters` sans `field`.

| Sortie | `result` | Raisons de `card keygen` et `card render` |
|---|---|---|
| 0 | | Fait. |
| 1 | `refused` | `period_not_calendar`, `period_not_closed`, `incomplete_period`, `volume_not_avoidable`, `threshold_not_applicable`, `unknown_equivalence`, `equivalence_out_of_scale`, `unsupported_language`. La carte annoncerait ce que ses chiffres ne montrent pas. |
| 3 | `input_error` | `missing_file`, `file_too_large`, `malformed_input`, `signing_key_permissions`, `signing_key_unreadable`, `key_exists`, `output_unwritable`. |
| 5 | `unrenderable` | `compact_too_large`, `unsupported_characters`, `unsupported_version`, `png_encode_failed`, `pdf_export_failed`, `svg_encode_failed`, `jxl_encode_failed`. |

| Sortie | Ligne et raisons de `verify-card` |
|---|---|
| 0 | `integrity` `ok` et `content` `intact` (un fichier) ou `not_checked` (une URL). Niveau 2 : `key_published`. Niveau 1 : `no_key_list`, `key_not_published`. |
| 1 | `integrity` `tampered` (`signature_invalid`, `fingerprint_mismatch`), un fichier qui n'est pas celui qui a été signé (`content_modified`), ou une clé dont la liste ne se porte pas garante (`key_revoked`, `signed_outside_validity`, `public_key_mismatch`), au niveau 1. |
| 3 | `{"result":"input_error"}` : `missing_file`, `file_too_large`, `malformed_input`, `no_card_payload`, `duplicate_card_payload`, `no_domain`. |
| 4 | `--online` n'a pas pu lire les clés du domaine : la ligne de vérification au niveau 1 avec `binding_check_failed` ou `binding_partial`, ou `no_http_client` sur une compilation sans les features HTTP. |
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
| `story` | sur une histoire seulement | `{"finding_type","service","endpoint","observed_at","done_at"}`. `observed_at` est facultatif. `service` et `endpoint` tiennent chacun en 200 caractères. `done_at` tombe dans la période ou avant, `observed_at` le jour de `done_at` ou avant. |
| `before_fix_period` | quand la carte en imprime une | `{"from","to"}`, une période civile de type `period.kind`, voir [la période d'avant la correction](#la-période-davant-la-correction). |
| `equivalence` | non | Un `id` de la table des équivalences. Absent, le moteur en choisit une. |
| `free` | oui, `{}` si vide | `{"title","team","label"}`, chacun facultatif : titre 48 caractères, équipe 48, libellé 80 (sur une histoire seulement). Un titre ne peut pas contenir `{"content_sig":"`, le marqueur de l'emplacement de signature du PDF : `{"result":"input_error","reason":"malformed_input","field":"free.title"}`, sortie 3, quel que soit le format. |
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

- **Enveloppe de fichier** : `{"content_sig":"<base64url de la signature Ed25519 de 64 octets sur le contenu du fichier>","payload":"<base64url de la charge utile complète canonique>","signature":"<base64url de la signature Ed25519 de 64 octets sur la charge utile>"}`, sans autre clé, écrite exactement ainsi (voir [Ce que couvre la signature du fichier](#ce-que-couvre-la-signature-du-fichier)). Une enveloppe sans `content_sig` donne `malformed_input`. Le PNG la porte dans un chunk `iTXt` au mot-clé `perf-sentinel-card`, avant les données d'image. Le PDF la porte en pièce jointe `perf-sentinel-card.json`. Le SVG la porte comme texte de `<metadata id="perf-sentinel-card">`, écrite telle quelle. Le JPEG XL la porte comme charge d'une boîte `pscd` de premier niveau, avant le codestream. L'enveloppe tient en 64 Kio. Un second chunk `perf-sentinel-card` avant les données d'image, une seconde pièce jointe de ce nom, un second élément de ce type, ou une seconde boîte `pscd` de premier niveau, donne `duplicate_card_payload`.
- **URL compacte** : `https://perf-sentinel.dev/verifier#c=<base64url de la charge utile compacte canonique>.<base64url de sa signature>`. Le jeton voyage dans le fragment, qu'un navigateur n'envoie jamais au serveur. Un lecteur prend ce qui suit le premier `#c=` et le coupe sur son dernier `.`.

### Règles d'encodage

- **Octets canoniques.** Une charge utile se signe en JSON, clés triées à chaque niveau, sans espaces, chaînes en UTF-8 telles qu'écrites (seuls `"`, `\` et les caractères de contrôle sont échappés, et les textes n'en contiennent aucun). Un vérificateur vérifie la signature sur les octets que porte la carte et ne les resérialise jamais.
- **base64url strict.** L'alphabet `A-Z a-z 0-9 - _`, sans remplissage, une longueur qui ne vaut pas 1 modulo 4, et des bits de fin à zéro, pour qu'un seul caractère modifié ne décode jamais vers les mêmes octets.
- **Empreinte.** `ps1:` puis le base64url des 16 premiers octets du SHA-256 de la clé publique brute de 32 octets : 26 caractères.
- **Entiers seulement.** Chaque nombre est un entier de 0 à 2^53 − 1, l'unité dans le nom de la clé (`_ug`, `_ops`, `_ms`, `_permille`, `_count`, `_days`). Un flottant, un nombre négatif ou un entier plus grand donne `malformed_input`.
- **Signature.** Ed25519 (RFC 8032) sur les octets canoniques. Elle est déterministe : une charge utile et une clé donnent une seule signature.

### Ce que couvre la signature du fichier

`content_sig` est une seconde signature Ed25519 par la même clé, sur les octets du fichier lui-même moins le support de l'enveloppe. La charge utile ne peut pas porter l'empreinte du fichier : l'image dessine le QR code, qui contient la signature compacte, si bien qu'une charge utile qui nommerait l'empreinte de l'image se nommerait elle-même.

**Le message.** `content_sig` signe ces lignes ASCII, l'hexadécimal en minuscules, sans saut de ligne après la dernière :

```text
perf-sentinel-card-content-v1
<format>
<SHA-256 en hexadécimal de la charge utile complète canonique>
<SHA-256 en hexadécimal du contenu>
```

`<format>` vaut `png`, `svg`, `jxl` ou `pdf`. La première ligne distingue le message d'une charge utile, qui commence toujours par `{`. Le format empêche une enveloppe de passer à un autre format de la même carte. L'empreinte de la charge utile empêche l'échange entre deux vraies cartes d'une même clé : sans elle, la charge utile de la carte B à côté du fichier de la carte A et du `content_sig` de A donnerait « intact » alors que les pixels montrent A. Vecteur de test, format `png`, charge utile `{}`, contenu `abc` :

```text
perf-sentinel-card-content-v1
png
44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a
ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
```

**Le texte de l'enveloppe est vérifié octet par octet.** Le contenu laisse de côté tout le support, si bien qu'aucune signature ne couvre le texte de l'enveloppe, et un analyseur JSON lit beaucoup de textes comme une seule valeur. Un lecteur décode les trois valeurs, réécrit l'enveloppe à partir d'elles et exige les mêmes octets, sinon `malformed_input`. Sans cette règle, un vrai PNG dont le texte `iTXt` reçoit ses clés dans un autre ordre, une espace après un deux-points, un saut de ligne final ou un échappement `\u0041` (CRC recalculé) garderait son contenu et donnerait « intact ». Il en va de même pour chaque format.

**Le contenu, par format**, lu sur les octets bruts, sans rien décoder :

- **PNG** : le fichier sans le chunk `iTXt` de la carte, retiré en entier (longueur, type, données et CRC), trouvé par le parcours PNG décrit plus bas.
- **SVG** : le fichier sans les octets qui vont de `<metadata id="perf-sentinel-card">` au `</metadata>` suivant (le saut de ligne qui le suit reste). L'élément doit se trouver là où le moteur l'écrit, juste après le premier `</title>` et son saut de ligne, sinon `malformed_input`. Le titre est échappé pour XML, son texte ne contient donc jamais `</title>`.
- **JPEG XL** : le fichier sans la boîte `pscd`, retirée en entier (taille, type et charge). La boîte doit se trouver là où le moteur l'écrit : les boîtes de premier niveau commencent par la boîte de signature, `ftyp`, `pscd` et `jxlc`, dans cet ordre, la boîte `pscd` à l'offset 32 avec une taille sur 32 bits (ni 0, ni 1). `verify-card` lit encore l'enveloppe d'une boîte `pscd` placée ailleurs, puis refuse le fichier avec `malformed_input`.
- **PDF** : tout le fichier, les 86 octets de la valeur de `content_sig` remis à 86 `A`, le base64url de 64 octets nuls. Une recherche d'octets trouve cet emplacement : `{"content_sig":"` apparaît exactement une fois dans le fichier brut, les 86 octets qui suivent égalent le `content_sig` de l'enveloppe, et `","payload":"` les suit. Tout autre cas donne `content_modified`.

Un élément SVG ou une boîte JPEG XL déplacés sont refusés plutôt que hachés : retirer le support de n'importe quel endroit laisse les mêmes octets, si bien que l'empreinte seule ne voit pas qu'il a bougé. Un élément SVG déplacé dans `<title>` ou dans l'attribut `d` d'un chemin donnerait « intact » alors que le fichier se nomme ou se dessine autrement.

Le PDF utilise un emplacement fixe plutôt qu'une enveloppe dans une mise à jour incrémentale ajoutée à la fin. Une mise à jour demanderait une révision PDF écrite à la main (un nouveau catalogue, les entrées `/AF` et `/EmbeddedFiles` qu'exige PDF/A-3, une section xref avec `/Prev`), et une règle pour distinguer la mise à jour du moteur d'une autre ajoutée après elle. L'emplacement couvre tous les octets sauf 86, une mise à jour ajoutée comprise, et reste une recherche d'octets qu'un navigateur sait faire.

**Le parcours PNG de la carte.** Un seul jeu de règles d'octets, qu'un second lecteur suit tel qu'écrit :

1. Les 8 premiers octets valent `89 50 4E 47 0D 0A 1A 0A`, sinon `malformed_input`.
2. À partir de l'offset 8, chaque chunk est une longueur sur 32 bits gros-boutiste, un type de 4 octets, les données et un CRC de 4 octets. Une longueur au-dessus de `0x7FFFFFFF`, un chunk qui dépasse la fin du fichier ou un en-tête coupé donnent `malformed_input`, et aucun tampon n'est dimensionné d'après un champ de longueur.
3. Le premier chunk est `IHDR`, de longueur 13, sinon `malformed_input` : un PNG de carte commence comme tout PNG.
4. Le parcours s'arrête au premier `IEND`, et un fichier sans `IEND` donne `malformed_input`. Les octets après `IEND` ne sont pas parcourus mais font partie du contenu, si bien que des octets ajoutés à un original donnent `content_modified`, jamais une erreur de format et jamais un oubli.
5. Le chunk de la carte est un `iTXt` dont les données commencent par `perf-sentinel-card` et un NUL. Aucun dans le parcours : `no_card_payload`. Deux ou plus, avant ou après `IDAT` : `duplicate_card_payload`. Le seul placé après le premier `IDAT` : `no_card_payload`, puisqu'un décodeur qui s'arrête aux données d'image ne le voit jamais.
6. Le CRC du chunk de la carte (CRC-32 sur son type et ses données) correspond. Les quatre octets qui suivent le NUL du mot-clé valent `00 00 00 00` : pas de compression, méthode 0, une étiquette de langue vide et un mot-clé traduit vide. Le texte, à partir de l'octet 23 des données, est en UTF-8 et fait au plus 64 Kio. Tout autre cas donne `malformed_input`. Ces octets d'en-tête se trouvent dans le chunk exclu, qu'aucune signature ne couvre, si bien qu'une étiquette de langue au contenu libre laisserait un fichier qui donne « intact » porter des données non signées, jusqu'au plafond de 16 Mio. Le texte est stocké sans compression pour que le support garde sa longueur entre le brouillon et le fichier scellé.

Les CRC des autres chunks ne sont pas vérifiés : ils font partie du contenu, si bien qu'un CRC modifié là donne `content_modified`. La place du chunk de la carte parmi les chunks entre `IHDR` et le premier `IDAT` n'est pas signée, voir [`LIMITATIONS-FR.md`](./LIMITATIONS-FR.md#cartes-à-partager).

**Brouillon, empreinte, signature, assemblage.** `card render` écrit le fichier avec un `content_sig` provisoire (86 `A`, la longueur d'une vraie valeur), prend son contenu par la fonction qu'utilise `verify-card`, signe le message, puis écrit la vraie valeur. Un PNG est encodé deux fois, un SVG écrit deux fois, un codestream JPEG XL encodé une fois et ses boîtes écrites deux fois. Le PDF est exporté une seule fois et la vraie valeur écrite sur la valeur provisoire, sur place : krilla tire le `/ID` du trailer et le `xmpMM:InstanceID` du XMP des octets qu'il écrit, si bien qu'un second export avec une autre enveloppe changerait des octets hors de l'emplacement. La longueur ne change pas, aucun offset de la xref ne bouge, et les `/Params` de la pièce jointe ne contiennent que `/Size` et `/ModDate`, sans somme de contrôle. La même entrée et la même clé donnent les mêmes octets dans chaque format.

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

**Rotation et révocation.** Une clé est valide de `from` à `to`, bornes comprises, comparées au jour UTC où la carte a été signée (`generated_at_ms`, ou `g`). Ce jour est une affirmation du signataire, écrite par qui détient la clé.

| La liste dit | Carte signée | Résultat |
|---|---|---|
| Rien (aucune liste lue) | | Niveau 1, `unknown`, `no_key_list` |
| Pas cette empreinte | | Niveau 1, `unknown`, `key_not_published` |
| Une autre `public_key` pour cette empreinte | | Niveau 1, `unknown`, `public_key_mismatch`, sortie 1 |
| `revoked` | n'importe quel jour | Niveau 1, `revoked`, `key_revoked`, sortie 1 |
| Valide, sans `to` | avant `from` | Niveau 1, `current`, `signed_outside_validity`, sortie 1 |
| Valide, avec `to` | hors de `from`..`to` | Niveau 1, `retired`, `signed_outside_validity`, sortie 1 |
| Valide, sans `to` | dans sa validité | Niveau 2, `current`, `key_published` |
| Valide, avec `to` | dans sa validité | Niveau 2, `retired`, `key_published` |

Pour changer de clé, publiez la nouvelle avec son `from` et posez `to` sur l'ancienne : les cartes signées avec l'ancienne restent au niveau 2, `retired`. Cette fenêtre repose sur la date du signataire : une clé volée qui est seulement retirée peut encore dater une carte à l'intérieur. Pour écarter une clé qui a pu fuiter, posez `revoked` : toute carte qu'elle a signée tombe au niveau 1, `key_revoked`, quelle que soit la date que la carte affiche, car une clé volée pourrait dater une carte d'avant n'importe quel jour de révocation. Les cartes honnêtes signées par cette clé perdent aussi le niveau 2, et aucun horodatage tiers ne les distingue. Révoquez une clé qui a pu fuiter, retirez celle qui n'a pas fui. Le jour `revoked` est le premier jour où la clé a pu fuiter, pas le jour où elle a été écartée : il ne change aucun verdict et indique seulement au lecteur depuis quand la clé a pu être exposée. Quand l'enregistrement DNS et le fichier well-known listent tous deux une clé, chaque limite posée par l'un ou l'autre s'applique : `revoked` l'emporte sur `to`, et `to` sur l'absence de limite.

## `--online`

`verify-card --online` lit le domaine de la carte (`org.domain`, ou `d`), une carte sans domaine donne donc `no_domain`. La commande interroge deux endroits en même temps, par le client HTTP partagé du moteur :

1. Les enregistrements TXT de `_perf-sentinel.<domaine>`, auprès d'un service DNS-over-HTTPS JSON (`application/dns-json`) : `https://cloudflare-dns.com/dns-query` par défaut, ou l'URL `https://` que nomme `--doh`. Le moteur n'interroge qu'un domaine qui respecte la règle de domaine de la carte, et ne lit une réponse que si sa question et ses enregistrements TXT nomment `_perf-sentinel.<domaine>`, à la casse et à un point final près. Un enregistrement TXT atteint par des enregistrements CNAME présents dans la même réponse compte : le moteur suit cette chaîne dans la réponse, sur 8 sauts au plus. Une réponse dont les noms ne se relient pas ainsi à `_perf-sentinel.<domaine>` se lit « vérification impossible ».
2. `https://<domaine>/.well-known/perf-sentinel-key`, sur le domaine lui-même, sans port ni partie utilisateur.

Chaque requête dispose de 5 s et lit 64 Kio au plus. Aucune redirection n'est suivie : un domaine dont l'apex redirige vers `www` fait échouer la lecture du well-known, publiez donc le fichier sur l'apex, ou comptez sur l'enregistrement DNS. Un 404, un NXDOMAIN ou l'absence d'enregistrement `v=ps1` signifient « ne publie rien ». Un délai dépassé, un statut d'erreur, une redirection ou un corps qui n'a pas la forme attendue signifient « vérification impossible ». Les deux réponses se fusionnent par empreinte. Quand une requête n'a pas pu vérifier et que l'autre n'a trouvé aucune clé, le résultat est `binding_check_failed` au niveau 1, sortie 4 : « vérification impossible » ne se lit jamais « non publiée », puisque la réponse manquante a pu révoquer la clé. Quand une requête n'a pas pu vérifier et que l'autre a trouvé la clé, le résultat est `binding_partial` au niveau 1, sortie 4, pour la même raison : la source non lue est peut-être celle qui révoque la clé. Un NXDOMAIN ou un 404 est une réponse, pas un échec, donc un domaine qui publie à un seul endroit atteint quand même le niveau 2.

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
| Collectif | `{n} corrections confirmées par la mesure, sur {s} services`, suivi d'une virgule puis de `contre {p} le mois précédent` (ou la semaine précédente, le trimestre précédent, le semestre précédent, l'année précédente) quand le Hub a compté la période précédente |
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

Dans la ligne `Mesuré avec`, les deux noms Perf Sentinel et Institut du Numérique Responsable s'impriment en gras, soulignés, dans le vert de la marque `#107a56`, et chacun reste entier sur une ligne. Les mots sont ceux du tableau.

En français, une espace insécable sépare un chiffre de son unité, les groupes de milliers, et précède `:` et `%`. Chaque chiffre est arrondi vers le bas : les nombres sont exacts sous 10 000, puis gardent deux chiffres significatifs (« 1,9 million »), les durées sont en millisecondes entières sous 100 ms, puis en secondes avec une décimale, et une part sous 0,1 % se lit « moins de 0,1 % ». Les unités que compte une équivalence sont `repas moyen(s)`, `km en TGV`, `km parcourus par une voiture moyenne` et `smartphone(s)`.

La ligne CO2 d'une histoire couvre la seule période d'avant la correction, celle d'une courbe la période de la carte, et celle d'un collectif la période d'avant la correction de ses seules corrections N+1 et redondantes. La ligne de périmètre d'une courbe compte les jours de toutes les périodes qu'elle montre, de la première barre à la dernière.

## Le SVG

`card.svg` dessine les mêmes opérations que `card.png`. Son texte est fait des contours des glyphes des polices DM Sans embarquées, un `<path>` par morceau de texte, et non d'un élément `<text>` avec une police intégrée :

1. Le SVG dessine exactement ce que la mise en page a mesuré. Le moteur de texte d'une visionneuse applique le crénage et ses propres métriques, si bien qu'une ligne mesurée pour tenir pourrait déborder, ou passer à la ligne là où le PNG ne le fait pas.
2. La visionneuse n'a besoin d'aucune police. Plusieurs éditeurs ignorent un `@font-face` intégré et le remplacent par une police système, dont la mise en page n'a jamais vérifié les glyphes.
3. Les octets ne dépendent que de la carte et des polices embarquées, jamais d'une police installée chez le lecteur.
4. Intégrer les deux fichiers DM Sans ajouterait environ 143 Kio avant le moindre glyphe, et les réduire aux glyphes utiles demande une crate que le moteur n'a pas.

Le prix : le texte ne se sélectionne pas et ne se recherche pas, et un lecteur d'écran lit le `<title>` comme nom de l'image. Ce titre est le `free.title` de la carte, sinon `Carte Perf Sentinel` (`Perf Sentinel card` en anglais), comme dans les métadonnées du PDF. DM Sans est sous licence SIL Open Font License, qui autorise les deux formes.

Le fichier commence par un en-tête fixe :

```xml
<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="1080" height="1350" viewBox="0 0 1080 1350" xml:lang="fr" role="img">
<title>{titre, échappé en XML}</title>
<metadata id="perf-sentinel-card">{enveloppe}</metadata>
```

Suivent une ligne par opération de dessin, dans l'ordre du PNG, puis `</svg>`. Les seuls éléments sont `svg`, `title`, `metadata`, `defs`, `linearGradient`, `stop`, `path`, `rect` et `circle` : ni script, ni style, ni lien, ni image, ni `<text>`. Les nombres sont en virgule fixe avec deux décimales au plus, si bien que la même carte donne les mêmes octets sur une même cible de compilation.

L'enveloppe ne contient que de la ponctuation JSON (`{`, `}`, `"`, `:`, `,`) et des caractères base64url, elle n'a donc besoin d'aucun échappement XML. `card render` vérifie chaque octet avant d'écrire et s'arrête sinon sur `svg_encode_failed`, ce que l'enveloppe canonique ne déclenche jamais. `verify-card` lit l'enveloppe sans analyseur XML : le moteur écrit l'élément sous une seule forme, et un analyseur serait une dépendance de plus et davantage de code exposé à un fichier envoyé par un inconnu. Il cherche la chaîne exacte `<metadata id="perf-sentinel-card">` et prend son texte jusqu'au `</metadata>` suivant :

| Le fichier | Raison |
|---|---|
| Plus de 16 Mio | `file_too_large` |
| Pas en UTF-8, ou ne commence pas par `<?xml` ou `<svg` après un BOM UTF-8 facultatif et des blancs | `malformed_input` |
| Aucun élément de ce type | `no_card_payload` |
| L'élément deux fois | `duplicate_card_payload` |
| Pas de `</metadata>`, plus de 64 Kio, ou un octet hors de l'ensemble ci-dessus | `malformed_input` |
| L'élément ailleurs que juste après le premier `</title>` et son saut de ligne | `malformed_input` |

Sur un fichier original, le `content_sig` de l'enveloppe couvre aussi le dessin, voir [Ce que couvre la signature du fichier](#ce-que-couvre-la-signature-du-fichier).

Tailles mesurées sur les fixtures de test, enveloppe comprise :

| Fixture | Anglais | Français |
|---|---|---|
| Histoire | 319 Kio | 324 Kio |
| Courbe | 298 Kio | 293 Kio |
| Collectif | 325 Kio | 338 Kio |

## Le JPEG XL

`card.jxl` contient les pixels de `card.png`, sans perte : le moteur rastérise la carte une fois et passe les mêmes octets RVB 8 bits aux deux encodeurs, si bien qu'un `card.jxl` décodé égale un `card.png` décodé, octet pour octet. L'encodeur est jixel (pur Rust, sans dépendance, features par défaut et SIMD désactivées), en vitesse `Medium` sur un seul thread, pour que les octets ne dépendent pas du nombre de cœurs de la machine. Un test décode le fichier avec jxl-rs, le décodeur que livrent Chrome et Firefox, et avec jxl-oxide, un second décodeur indépendant, et compare les deux aux pixels du PNG.

Tailles mesurées sur les fixtures de test (anglais, enveloppe comprise) :

| Fixture | `card.jxl` | `card.png` | Rapport |
|---|---|---|---|
| Histoire | 44 822 o | 112 089 o | 0,40 |
| Courbe | 41 486 o | 110 649 o | 0,37 |
| Collectif | 42 114 o | 116 822 o | 0,36 |

Le JPEG XL est un export d'archive ou de haute qualité. Le PNG reste le format à publier : LinkedIn n'accepte pas le JPEG XL ([sa page d'aide](https://www.linkedin.com/help/linkedin/answer/a564109) liste GIF, HEIF/HEIC sous iOS, JPEG, PNG et WEBP pour les images), et la prise en charge des navigateurs est récente et inégale : Chrome l'active par défaut depuis la version 155, Firefox depuis la 158, Safari la prend en charge partiellement depuis la 17, et Edge ne l'affiche pas ([caniuse](https://caniuse.com/jpegxl), vérifié le 2026-10-30).

jixel écrit un codestream nu (il commence par `FF 0A`) et n'a pas d'API pour une boîte personnalisée, le moteur écrit donc le conteneur lui-même. La disposition est fixe :

```text
00 00 00 0C  4A 58 4C 20  0D 0A 87 0A         boîte de signature
00 00 00 14  ftyp  jxl   00 00 00 00  jxl     boîte ftyp
taille(8 + n)  pscd  <n octets d'enveloppe>   la boîte de la carte
taille(8 + m)  jxlc  <m octets de codestream> le codestream
```

Les tailles sont sur 32 bits gros-boutiste et comptent l'en-tête de 8 octets. Rien ne suit `jxlc`. La charge de `pscd` est l'enveloppe de fichier telle quelle, les mêmes octets que le texte `iTXt` du PNG. Elle précède le codestream, un lecteur la trouve donc dans les premières centaines d'octets.

`pscd` est un type de boîte privé. Le `decode.h` de libjxl permet à une application d'ajouter une boîte dont le type ne commence ni par `jxl` ni par `JXL` et ne reprend pas un type défini. jxl-rs et jxl-oxide la sautent, et le test ci-dessus décode la carte avec sa boîte `pscd`. Une boîte `xml ` imposerait un paquet XMP avec échappement XML, une seconde forme canonique que les outils de métadonnées réécrivent. Une boîte `Exif` est une structure TIFF, la boîte que les chaînes de traitement retirent le plus souvent. Une boîte `jumb` (JUMBF, le conteneur de C2PA) est bien plus lourde qu'il ne faut pour une carte.

`verify-card` lit l'enveloppe en parcourant les boîtes de premier niveau, sans décoder le codestream : un décodeur serait une grosse dépendance exposée à un fichier envoyé par un inconnu, pour des octets dont le lecteur n'a jamais besoin. Une boîte de taille 1 porte une taille sur 64 bits après son type, une boîte de taille 0 court jusqu'à la fin du fichier. Toute boîte autre que `pscd` est sautée selon sa longueur et jamais lue, une boîte `pscd` imbriquée dans une autre n'est donc pas vue.

| Le fichier | Raison |
|---|---|
| Plus de 16 Mio | `file_too_large` |
| Un codestream nu (commence par `FF 0A`) | `no_card_payload` |
| Ne commence pas par la boîte de signature | `malformed_input` |
| Une boîte plus petite que son en-tête, qui dépasse la fin du fichier, ou coupée dans son en-tête | `malformed_input` |
| Aucune boîte `pscd` de premier niveau | `no_card_payload` |
| Deux boîtes `pscd` de premier niveau | `duplicate_card_payload` |
| Une charge de `pscd` de plus de 64 Kio ou pas en UTF-8 | `malformed_input` |

`verify-card` exige ensuite la boîte `pscd` là où le moteur l'écrit et vérifie la signature du contenu du fichier, voir [Ce que couvre la signature du fichier](#ce-que-couvre-la-signature-du-fichier).

## QR code

Le QR code est en version 12 au plus, correction L : la carte vit sur un écran, et l'URL compacte ne tient pas en version 12 au niveau M. La version 12-L contient 367 octets. Le préfixe et la signature prennent 124 caractères de l'URL, ce qui laisse environ 182 octets au JSON compact. Avec les chiffres des fixtures, le plus long domaine qui tient fait 35 caractères sur une histoire avec `r`, 42 sur un collectif et 58 sur une courbe. Des volumes à dix chiffres ramènent l'histoire à 28 caractères. Un domaine plus long donne `compact_too_large`, jamais tronqué : le Hub doit donc vérifier la longueur avant de proposer le domaine.

## Lien vers la méthode

La dernière page du carrousel imprime `Méthode :` et l'adresse de cette page sur GitHub (`.../docs/FR/SHARE-CARD-FR.md`, ou `.../docs/SHARE-CARD.md` sur une carte en anglais), coupée seulement après un `/`. L'image imprime la page de vérification, `perf-sentinel.dev/verifier`, qui renvoie ici.

Les limites de ce que prouve une carte sont listées dans [`LIMITATIONS-FR.md`](./LIMITATIONS-FR.md#cartes-à-partager).
