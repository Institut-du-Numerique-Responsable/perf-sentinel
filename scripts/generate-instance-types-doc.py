#!/usr/bin/env python3
"""Generate the instance-type reference doc from the embedded power table.

The table is itself generated (`scripts/refresh-instance-power.py`), so the
doc is generated too rather than transcribed: 400+ rows copied by hand would
drift the first time the table is refreshed. `instance_types_doc_matches_the_table`
in `cloud_energy/table.rs` fails the build when the two disagree.

Usage: python3 scripts/generate-instance-types-doc.py
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SOURCES = [
    ROOT / "crates/sentinel-core/src/score/cloud_energy/table_data.rs",
    ROOT / "crates/sentinel-core/src/score/cloud_energy/table.rs",
]
ROW = re.compile(r'^\s*\("([^"]+)",\s*([0-9.]+),\s*([0-9.]+)\)', re.M)
VINTAGE = re.compile(r'SPECPOWER_VINTAGE: &str = "([^"]+)"')


def provider(name):
    """Classify the naming schemes used by the embedded table.

    Scaleway offer ids are the only all-upper-case, dash-separated ones
    (`COMPUTE3-X2C-4G`, `POP2-HM-2C-16G`), which the GCP fallback at the
    bottom would otherwise catch. Matched on that shape rather than on a
    list of range prefixes. `refresh-instance-power.py` reads the offer
    list live from the Product Catalog, so a new range lands in the table
    without anyone editing this file. A prefix list would file it under
    GCP silently.
    """
    if name.startswith("xeon-"):
        return "Bare metal"
    if name.startswith("Standard_"):
        return "Azure"
    if "-" in name and "." not in name and name == name.upper():
        return "Scaleway"
    if "." in name:
        return "AWS"
    return "GCP"


def collect():
    rows, seen = [], set()
    for path in SOURCES:
        for name, idle, mx in ROW.findall(path.read_text()):
            if name in seen:
                continue
            seen.add(name)
            rows.append((name, float(idle), float(mx)))
    return rows


def table(rows, headers):
    out = [f"| {headers[0]} | {headers[1]} | {headers[2]} |", "|---|---:|---:|"]
    for name, idle, mx in sorted(rows):
        out.append(f"| `{name}` | {idle:g} | {mx:g} |")
    return "\n".join(out)


# Static on purpose: the rules live in `resolve_instance_class`
# (`cloud_energy/table.rs`). Two columns only, the doc test reads every
# `| name | number | number |` line as a table row.
MANAGED_EN = """## Managed service classes

A managed database, cache, broker or search class runs on a VM family
the table prices, so its name resolves to that family's row. The rules
apply in order and stop at the first row. An exact row is never
rewritten, and only the Azure SQL vCore rule derives a wattage, by
scaling a row per vCore.

| Name shape | Resolves to |
|---|---|
| `db.<type>` (RDS, Aurora provisioned) | `<type>` |
| `cache.<type>` (ElastiCache) | `<type>` |
| `kafka.<type>`, `express.<type>` (MSK provisioned, MSK Express brokers) | `<type>` |
| `mq.<type>` (Amazon MQ) | `<type>` |
| `<type>.search`, `<type>.elasticsearch` (OpenSearch) | `<type>` |
| AWS `<family>.<size>`, family ending in up to two of `d`, `n`, `b` | the family without them: `db.r6idn.large` to `r6i.large`, `c7gn.large` to `c7g.large` |
| Azure `Standard_<series><size><features>_v<k>`, features holding `d` | the same name without the `d`: `Standard_D4ds_v5` to `Standard_D4s_v5`, `Standard_D4ads_v5` to `Standard_D4as_v5` |
| `db-n1-standard-N`, `db-n1-highmem-N` (Cloud SQL) | `n1-standard-N`, `n1-highmem-N` |
| `db-perf-optimized-N-V` (Cloud SQL Enterprise Plus, N2 series) | `n2-highmem-V` |
| `GP_Gen5_N`, `BC_Gen5_N`, `HS_Gen5_N`, serverless `GP_S_Gen5_N` and `HS_S_Gen5_N` (Azure SQL vCore, standard-series) | N times the per-vCPU rate of `Standard_D64s_v3`, N from 1 to 1024 |

The AWS letters `d` (local instance store), `n` (network) and `b` (block
storage), and the Azure `d` (local temp disk), leave the processor
unchanged. The processor letters `a`, `g`, `i` and the AWS `z` and `e`
modifiers are never dropped, so `m5zn` and `x2iedn` stay unknown. The
strip is approximate where a modifier changes the CPU generation: `c5n`
is a Skylake part while the `c5` rows are Cascade Lake, a gap inside the
2x bracket. A class whose base family has no row stays unknown.

Graviton 2 (`m6g`, `c6g`, `r6g`, `t4g`) and memory-optimized Graviton
(`r6g`, `r7g`, `r8g`) have rows, so `db.r6g.large`, `cache.t4g.micro` or
`db.m6gd.large` resolve. The Azure flexible-server Burstable classes are
the `Standard_B*` rows themselves, priced at their full vCPU since burst
credit is not modeled.

Enterprise Plus runs on the N2 series at 8 GB per vCPU, the shape of
`n2-highmem`. Azure SQL standard-series (Gen5) runs on several
processors (Broadwell, Skylake, Cascade Lake, Ice Lake, Milan, Genoa,
Emerald Rapids). Among those the Azure CSV publishes, Skylake has the
highest max W/vCPU, so the reference is the largest Skylake row,
`Standard_D64s_v3`: `GP_Gen5_64` equals it and a smaller objective
scales it per vCore, unrounded. Gen5 carries 5.1 GB per vCore, below the
DRAM premium threshold. A serverless objective is priced at its max
vCores, a ceiling. Replicas are nodes: Multi-AZ standbys, Aurora
readers and the Business Critical replicas count in `nodes`.

Not modeled. The startup warning, or the load error in
`[green.database_static]` and `[green.broker_static]`, names the reason
for the first five rows, the others are reported as unknown types:

| Class | Reason |
|---|---|
| `db.serverless` (Aurora Serverless v2) | sized in ACUs, each about 2 GiB of memory with corresponding CPU and networking, no published vCPU equivalence |
| `db-f1-micro`, `db-g1-small` (Cloud SQL shared core) | a shared vCPU, excluded like every shared-vCPU offer |
| `redis-*` (Memorystore node types) | the vCPU count is published, the machine series is not |
| `db-custom-V-MiB` (Cloud SQL dedicated core) | Cloud SQL names no machine series or processor behind it |
| `GP_Fsv2_N` and the other Azure SQL Fsv2 objectives | the hardware was retired on October 1, 2026 |
| Azure SQL DTU objectives (`S0`, `P1`), premium-series (`HS_PRMS_N`), DC-series (`GP_DC_N`) | no published processor mapping for the objective |
| AWS `x`, `z`, `i` and `im` families, OpenSearch `or1` | no row for the family: 16 to 32 GiB per vCPU beyond the binary DRAM premium rule, an undisclosed platform, or a storage-optimized or high-frequency family the table does not carry |
| ElastiCache, MSK and OpenSearch Serverless, Azure Cache for Redis, Event Hubs, Service Bus | billed in capacity units, no instance to declare |

Rule sources: the AWS [instance type naming
conventions](https://docs.aws.amazon.com/ec2/latest/instancetypes/instance-type-names.html),
the Azure [VM naming
conventions](https://learn.microsoft.com/en-us/azure/virtual-machines/vm-naming-conventions),
the [Aurora Serverless v2
capacity](https://docs.aws.amazon.com/AmazonRDS/latest/AuroraUserGuide/aurora-serverless-v2.how-it-works.html)
definition, the Cloud SQL [machine
series](https://docs.cloud.google.com/sql/docs/mysql/machine-series-overview),
the Azure SQL [vCore hardware
configurations](https://learn.microsoft.com/en-us/azure/azure-sql/database/service-tiers-sql-database-vcore),
and the shared-vCPU exclusion of `scripts/refresh-instance-power.py`.
"""

MANAGED_FR = """## Classes de services managés

Une classe managée de base de données, de cache, de broker ou de
recherche tourne sur une famille de VM que la table valorise, donc son
nom se résout sur la ligne de cette famille. Les règles s'appliquent dans
l'ordre et s'arrêtent à la première ligne trouvée. Une ligne exacte n'est
jamais réécrite, et seule la règle vCore Azure SQL calcule une
puissance, en mettant une ligne à l'échelle par vCore.

| Forme du nom | Se résout sur |
|---|---|
| `db.<type>` (RDS, Aurora provisionné) | `<type>` |
| `cache.<type>` (ElastiCache) | `<type>` |
| `kafka.<type>`, `express.<type>` (MSK provisionné, brokers MSK Express) | `<type>` |
| `mq.<type>` (Amazon MQ) | `<type>` |
| `<type>.search`, `<type>.elasticsearch` (OpenSearch) | `<type>` |
| AWS `<famille>.<taille>`, famille finissant par au plus deux lettres parmi `d`, `n`, `b` | la famille sans elles : `db.r6idn.large` vers `r6i.large`, `c7gn.large` vers `c7g.large` |
| Azure `Standard_<série><taille><options>_v<k>`, options contenant `d` | le même nom sans le `d` : `Standard_D4ds_v5` vers `Standard_D4s_v5`, `Standard_D4ads_v5` vers `Standard_D4as_v5` |
| `db-n1-standard-N`, `db-n1-highmem-N` (Cloud SQL) | `n1-standard-N`, `n1-highmem-N` |
| `db-perf-optimized-N-V` (Cloud SQL Enterprise Plus, série N2) | `n2-highmem-V` |
| `GP_Gen5_N`, `BC_Gen5_N`, `HS_Gen5_N`, serverless `GP_S_Gen5_N` et `HS_S_Gen5_N` (Azure SQL vCore, série standard) | N fois le taux par vCPU de `Standard_D64s_v3`, N de 1 à 1024 |

Les lettres AWS `d` (stockage d'instance local), `n` (réseau) et `b`
(stockage bloc), et le `d` Azure (disque temporaire local), ne changent
pas le processeur. Les lettres de processeur `a`, `g`, `i` et les
modificateurs AWS `z` et `e` ne sont jamais retirés, donc `m5zn` et
`x2iedn` restent inconnus. Le retrait est approximatif quand un
modificateur change la génération de CPU : `c5n` est une puce Skylake
alors que les lignes `c5` sont Cascade Lake, un écart qui reste dans la
fourchette 2x. Une classe dont la famille de base n'a pas de ligne reste
inconnue.

Graviton 2 (`m6g`, `c6g`, `r6g`, `t4g`) et les Graviton optimisés mémoire
(`r6g`, `r7g`, `r8g`) ont des lignes, donc `db.r6g.large`,
`cache.t4g.micro` ou `db.m6gd.large` se résolvent. Les classes Burstable
des serveurs flexibles Azure sont les lignes `Standard_B*` elles-mêmes,
valorisées à leurs vCPU complets puisque le crédit de rafale n'est pas
modélisé.

Enterprise Plus tourne sur la série N2 à 8 Go par vCPU, la forme de
`n2-highmem`. La série standard (Gen5) d'Azure SQL tourne sur plusieurs
processeurs (Broadwell, Skylake, Cascade Lake, Ice Lake, Milan, Genoa,
Emerald Rapids). Parmi ceux que le CSV Azure publie, Skylake a le plus
haut max W/vCPU, donc la référence est la plus grande ligne Skylake,
`Standard_D64s_v3` : `GP_Gen5_64` lui est égal et un objectif plus petit
la met à l'échelle par vCore, sans arrondi. Gen5 porte 5,1 Go par vCore,
sous le seuil de la prime DRAM. Un objectif serverless est valorisé à
son maximum de vCores, un plafond. Les réplicas sont des nœuds : les
standbys Multi-AZ, les lecteurs Aurora et les réplicas Business Critical
comptent dans `nodes`.

Non modélisées. L'avertissement de démarrage, ou l'erreur de chargement
dans `[green.database_static]` et `[green.broker_static]`, donne la raison
pour les cinq premières lignes, les autres sont signalées comme types
inconnus :

| Classe | Raison |
|---|---|
| `db.serverless` (Aurora Serverless v2) | dimensionnée en ACU, chacune d'environ 2 Gio de mémoire avec le CPU et le réseau correspondants, sans équivalence vCPU publiée |
| `db-f1-micro`, `db-g1-small` (Cloud SQL à cœur partagé) | un vCPU partagé, exclu comme toute offre à vCPU partagés |
| `redis-*` (types de nœud Memorystore) | le nombre de vCPU est publié, la série de machine ne l'est pas |
| `db-custom-V-MiB` (Cloud SQL à cœur dédié) | Cloud SQL ne nomme ni la série de machine ni le processeur |
| `GP_Fsv2_N` et les autres objectifs Azure SQL Fsv2 | le matériel a été retiré le 1er octobre 2026 |
| Objectifs Azure SQL DTU (`S0`, `P1`), série premium (`HS_PRMS_N`), série DC (`GP_DC_N`) | aucune correspondance de processeur publiée pour l'objectif |
| Familles AWS `x`, `z`, `i` et `im`, OpenSearch `or1` | aucune ligne pour la famille : 16 à 32 Gio par vCPU au-delà de la règle binaire de prime DRAM, une plateforme non publiée, ou une famille optimisée stockage ou haute fréquence que la table ne porte pas |
| ElastiCache, MSK et OpenSearch Serverless, Azure Cache for Redis, Event Hubs, Service Bus | facturés en unités de capacité, aucune instance à déclarer |

Sources des règles : les [conventions de nommage des types
d'instance](https://docs.aws.amazon.com/ec2/latest/instancetypes/instance-type-names.html)
AWS, les [conventions de nommage des
VM](https://learn.microsoft.com/fr-fr/azure/virtual-machines/vm-naming-conventions)
Azure, la définition de la [capacité Aurora Serverless
v2](https://docs.aws.amazon.com/AmazonRDS/latest/AuroraUserGuide/aurora-serverless-v2.how-it-works.html),
les [séries de machine](https://docs.cloud.google.com/sql/docs/mysql/machine-series-overview)
Cloud SQL, les [configurations matérielles
vCore](https://learn.microsoft.com/fr-fr/azure/azure-sql/database/service-tiers-sql-database-vcore)
Azure SQL, et l'exclusion des vCPU partagés de
`scripts/refresh-instance-power.py`.
"""


def render(rows, vintage, lang):
    providers = ("AWS", "GCP", "Azure", "Scaleway", "Bare metal")
    by_provider = {p: [r for r in rows if provider(r[0]) == p] for p in providers}
    if lang == "en":
        head = f"""# Instance types with an embedded power profile

Every `instance_type` accepted by `[green.cloud.services]`,
`[green.database_static]` and `[green.broker_static]`, with the idle and maximum wattage the SPECpower
model interpolates between. {len(rows)} entries, table vintage
`{vintage}`. Managed database, cache, broker and search class names are
accepted too: each resolves to the row of the VM family it runs on, see
[Managed service classes](#managed-service-classes).

**An unlisted type is not an error in `[green.cloud.services]`.** Perf
Sentinel warns once at startup, naming the type, and the reason when it
is a managed class the model leaves out, then falls back to a
provider-level average, so the figure gets coarser. That warning is also
how you check your own type without reading this page. In
`[green.database_static]` and `[green.broker_static]` the same type is an
error that stops the load, since declared hosts have no other source to
fall back on. When your
hardware is absent and you know its draw, declare it directly instead,
which is exact rather than approximated:

```toml
[green.cloud.services]
"my-service" = {{ idle_watts = 45.0, max_watts = 120.0 }}
```

**Scaleway rows are derived, not published.** Cloud Carbon Footprint
publishes coefficients for AWS, GCP and Azure only. Scaleway publishes
no wattage either, but its Product Catalog API names the **exact CPU** of
every offer without authentication (`AMD EPYC 7543`, not a generic
family). Each offer is priced by the CCF coefficient for that CPU's
architecture, with the same vCPU-times-coefficient arithmetic as the
three above. This adds a single assumption, the one CCF already
makes between AWS and GCP: that a coefficient computed on hyperscaler
fleets transfers to comparable silicon elsewhere. Three groups of offers
are excluded rather than approximated:

- **Shared-vCPU ranges** (`DEV1`, `PLAY2`, `BASIC2`, `BASIC3`), where
  attributing a whole vCPU to one tenant overstates what it draws.
- **GPU offers** (`H100`, `L4`, `L40S`, `RENDER`), because the table
  models no accelerator for any provider, and an H100 alone outweighs
  the entire CPU budget.
- **AmpereOne and Granite Rapids ranges** (`STANDARD2`, `B300`), absent
  from the CCF CSVs.

**OVHcloud and OUTSCALE are still absent.** Sources checked as of
August 2026:

- **Boavizta** ([BoaviztAPI](https://github.com/Boavizta/boaviztapi)) is
  the only third-party base that reaches instance granularity, with 50
  OVHcloud sizes. Its files carry **no power column**, and the CPU a
  wattage would be derived from is Boavizta's own assumption, flagged
  `CPU not verified` on all 12 OVHcloud archetype rows. One
  credits a rack server with an `Intel Core i7-4940MX`, a mobile part,
  across two sockets. OVHcloud itself documents no CPU per range.
- **OUTSCALE** stops at (Region, service category): two regions, three
  categories, no instance type, no watt, no kWh, and zero occurrences in
  Boavizta.

On that hardware, measure instead: Alumet or Scaphandre read RAPL
directly and outrank every modeled figure.

Where the numbers come from, and why a family maps to a coefficient
rather than to a measured machine: [`METHODOLOGY.md`](./METHODOLOGY.md)
and `docs/design/05-GREENOPS-AND-CARBON.md`. Configuring the scraper:
[`CONFIGURATION.md`](./CONFIGURATION.md).

This page is generated from the embedded table by
`scripts/generate-instance-types-doc.py`, and a test fails the build if
the two disagree. Do not edit it by hand.
"""
        headers = ("Instance type", "Idle (W)", "Max (W)")
        units = ("entry", "entries")
        managed = MANAGED_EN
    else:
        head = f"""# Types d'instance avec un profil de puissance embarqué

Tous les `instance_type` acceptés par `[green.cloud.services]`,
`[green.database_static]` et `[green.broker_static]`, avec les puissances au repos et maximale entre
lesquelles le modèle SPECpower interpole. {len(rows)} entrées, millésime
de la table `{vintage}`. Les noms de classes managées de base de données,
de cache, de broker et de recherche sont acceptés aussi : chacun se résout
sur la ligne de la famille de VM qui l'héberge, voir
[Classes de services managés](#classes-de-services-managés).

**Un type absent n'est pas une erreur dans `[green.cloud.services]`.** Perf
Sentinel avertit une seule fois au démarrage en nommant le type, et la
raison quand il s'agit d'une classe managée que le modèle laisse de côté,
puis se rabat sur une moyenne du fournisseur, ce qui rend la valeur plus
grossière. Cet avertissement est aussi la façon de vérifier votre propre
type sans lire cette page. Dans `[green.database_static]` et
`[green.broker_static]`, le même type est une erreur qui arrête le
chargement, puisque des hôtes déclarés n'ont aucune autre source sur
laquelle se rabattre. Quand votre matériel est absent et
que vous connaissez sa consommation, déclarez-la directement, ce qui donne
une valeur exacte plutôt qu'approchée :

```toml
[green.cloud.services]
"my-service" = {{ idle_watts = 45.0, max_watts = 120.0 }}
```

**Les lignes Scaleway sont dérivées, pas publiées.** Cloud Carbon
Footprint ne publie des coefficients que pour AWS, GCP et Azure.
Scaleway ne publie pas non plus de puissance, mais son API Product
Catalog nomme le **CPU exact** de chaque offre sans authentification
(`AMD EPYC 7543`, et non une famille générique). Chaque offre est
valorisée par le coefficient CCF de l'architecture de ce CPU, selon la
même arithmétique vCPU fois coefficient que les trois autres. Cela
ajoute une seule hypothèse, celle que CCF pose déjà entre AWS et GCP :
qu'un coefficient calculé sur des flottes d'hyperscalers se transpose à
du silicium comparable ailleurs. Trois groupes d'offres sont exclus
plutôt qu'approchés :

- Les **gammes à vCPU partagés** (`DEV1`, `PLAY2`, `BASIC2`, `BASIC3`),
  où attribuer un vCPU entier à un locataire surestime ce qu'il tire.
- Les **offres GPU** (`H100`, `L4`, `L40S`, `RENDER`), parce que la
  table ne modélise aucun accélérateur chez aucun fournisseur, et
  qu'une H100 pèse à elle seule plus que tout le budget CPU.
- Les **gammes AmpereOne et Granite Rapids** (`STANDARD2`, `B300`),
  absentes des CSV de CCF.

**OVHcloud et OUTSCALE restent absents.** Sources consultées en août
2026 :

- **Boavizta** ([BoaviztAPI](https://github.com/Boavizta/boaviztapi))
  est la seule base tierce à descendre au type d'instance, avec 50
  tailles OVHcloud. Ses fichiers ne portent **aucune colonne de
  puissance**, et le CPU dont une puissance serait dérivée est une
  hypothèse de Boavizta lui-même, marquée `CPU not verified` sur les 12
  lignes d'archétype OVHcloud. L'une crédite un serveur rack d'un
  `Intel Core i7-4940MX`, une puce mobile, sur deux sockets. OVHcloud
  lui-même ne documente aucun CPU par gamme.
- **OUTSCALE** s'arrête au couple (Région, catégorie de service) : deux
  régions, trois catégories, aucun type d'instance, aucun watt, aucun
  kWh, et zéro occurrence chez Boavizta.

Sur ce matériel, mesurez plutôt : Alumet ou Scaphandre lisent RAPL
directement et priment sur toute valeur modélisée.

D'où viennent ces chiffres, et pourquoi une famille correspond à un
coefficient plutôt qu'à une machine mesurée :
[`METHODOLOGY-FR.md`](./METHODOLOGY-FR.md) et
`docs/FR/design/05-GREENOPS-AND-CARBON-FR.md`. Configurer le scraper :
[`CONFIGURATION-FR.md`](./CONFIGURATION-FR.md).

Cette page est générée depuis la table embarquée par
`scripts/generate-instance-types-doc.py`, et un test casse le build si les
deux divergent. Ne la modifiez pas à la main.
"""
        headers = ("Type d'instance", "Repos (W)", "Max (W)")
        units = ("entrée", "entrées")
        managed = MANAGED_FR

    parts = [head, managed]
    for name, entries in by_provider.items():
        count = len(entries)
        parts.append(f"## {name} ({count} {units[count != 1]})")
        parts.append("")
        parts.append(table(entries, headers))
        parts.append("")
    return "\n".join(parts)


def main():
    rows = collect()
    vintage = VINTAGE.search(SOURCES[0].read_text())
    if not vintage:
        print("could not read SPECPOWER_VINTAGE", file=sys.stderr)
        return 1
    for lang, path in (
        ("en", ROOT / "docs/INSTANCE-TYPES.md"),
        ("fr", ROOT / "docs/FR/INSTANCE-TYPES-FR.md"),
    ):
        path.write_text(render(rows, vintage.group(1), lang))
        print(f"wrote {path.relative_to(ROOT)} ({len(rows)} rows)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
