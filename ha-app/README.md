# Home Assistant app pre Genesis pilot

`ha-app/genesis/` obsahuje konfiguráciu a Dockerfile pre Home Assistant Green (`aarch64`). Obraz obsahuje predkompilovaný Rust core; na Green sa Rust ani Cargo neinštalujú. CI vytvára ARM64 image archive a skúša štart, health endpoint, chránený inventár, reštart a perzistenciu SQLite v emulovanom ARM64 kontajneri.

Home Assistant app používa `homeassistant_api: true` a interný WebSocket proxy `ws://supervisor/core/websocket`. Supervisor poskytne `SUPERVISOR_TOKEN` v prostredí; štartovací skript ho odovzdá core ako `GENESIS_HA_TOKEN`. Tento token sa neukladá do Gitu ani do možností app. Používateľ nastaví samostatné `read_token` a `write_token` (každý najmenej 32 znakov). Write token povoľuje pilotné povely a nesmie byť vystavený verejnému internetu. SQLite sa ukladá do perzistentného `/data/genesis-ledger.sqlite3`. Je to celý stav Genesis; záloha, obnova a rollback sú v [docs/ELYSIUM-348-obnova.md](../docs/ELYSIUM-348-obnova.md).

## Flutter panel v HA app

CI pred zostavením ARM64 obrazu zostaví a otestuje Flutter web panel. Obraz obsahuje statické súbory v `/usr/share/genesis-panel`. Od verzie `0.1.2` sa webové UI otvára cez **OPEN WEB UI** v Home Assistante, vrátane vzdialeného prístupu do HA. HA Ingress sprostredkuje prihlásenie a smeruje panel aj API pod jedným prefixom; Genesis naďalej vyžaduje vlastný prístupový token pre inventár a povely. Port 8765 sa v tejto verzii už nepublikuje na hostiteľa. Watchdog ostáva na `/health`. Zadaný prístupový token zostáva v pamäti otvoreného panelu a neposiela sa v URL. Samotný CI smoke test nie je dôkazom behu na Green ani fyzickom iPade.

## Stav distribúcie

Workflow [Build Genesis HA app (ARM64)](../.github/workflows/ha-app.yml) obraz zostaví, spustí naň smoke test a uloží `genesis-ha-pilot-arm64` artifact. Pri pushi do `main` ho **publikuje do GHCR** — a publikuje presne ten obraz, ktorý smoke testom prešiel, nie druhý build.

Meno obrazu aj verziu si workflow číta z `genesis/config.yaml`, pretože Supervisor sťahuje presne `<image>:<version>`; keby to bolo napísané na dvoch miestach, rozišlo by sa to. Z tej istej verzie sa plní aj `io.hass.version` v obraze a CI overuje, že značka a label hovoria to isté.

Publikujú sa dve značky:

- `sha-<commit>` — nemenná, vzniká pri každom pushi do `main`. Slúži na dohľadanie, čo presne beží.
- `<version>` — vydanie. **Nikdy sa neprepisuje.** Ak verzia v registri už je, workflow značku neposunie a napíše varovanie. Je to zámer: Supervisor rozhoduje o aktualizácii podľa čísla verzie, takže vymeniť obsah pod tým istým číslom znamená, že prevádzkovateľ opravu nikdy nedostane — jednotka si bude myslieť, že je aktuálna. Vydanie preto znamená zvýšiť `version` v `genesis/config.yaml`.

Obraz sa zostavuje aj pri zmene v `panel/`, nielen v `core/` a `ha-app/`. Panel je súčasťou obrazu, takže bez toho by sa jeho zmena overila na PR, ale po merge by sa do žiadneho obrazu nedostala — presne to sa stalo prekresleniu panelu do Elysium dizajnu, ktoré je v `main` od `c5bdd85`, ale vo verzii `0.1.2` nie je.

### Inštalácia a overenie

Repozitár aj GHCR balík `os-genesis-pilot` sú verejne dostupné na čítanie. Verziu `0.1.1` už Supervisor na Home Assistant Green stiahol; Ingress vo verzii `0.1.2` ešte treba overiť na skutočnom Green a cez vzdialené prihlásenie do HA. Verzia `0.1.3` prináša panel v Elysium dizajne; na skutočnom Green ani na fyzickom iPade ho zatiaľ nikto nevidel. Obnova po reštarte HA OS, skutočné zariadenie a merania CPU/RAM zostávajú otvorené akceptačné kroky Jira ELYSIUM-338 a ELYSIUM-336.

Verzia `0.1.4` prináša ELYSIUM-353 až ELYSIUM-357: skutočný stav povelu a ledger v paneli, miestnosti z Home Assistanta namiesto troch napísaných, podpísaný kanál z Behavior, párovanie a odobranie prístupu a prehľad časových prístupov s incidentmi. Je publikovaná (`sha256:97502fb4…`, tá istá ako nemenná `sha-34a1768`) — to znamená stiahnuteľná, nie overená: na skutočný Home Assistant Green ju zatiaľ nikto nenainštaloval.

Verzia `0.1.5` prináša ELYSIUM-358 až ELYSIUM-360: hlasový povel a potvrdenie citlivej akcie v paneli, prevádzkové centrum so zálohou a prehľadom stavu, a verzovaný iOS projekt. Je publikovaná (`sha256:cb9b0e0f…`, tá istá ako nemenná `sha-556c43d`) — to znamená stiahnuteľná, nie overená: na skutočný Home Assistant Green ju zatiaľ nikto nenainštaloval. ELYSIUM-361 (odmietnutie nešifrovaného spojenia mimo lokálnej siete) je v `main` až po `0.1.5`, takže vo vydanej verzii nie je.

Pri `0.1.5` som znova prešiel, čo core číta z prostredia a čo mu app nastavuje. Tentoraz nič tmavé nepribudlo: jediná premenná, ktorú `run.sh` nenastavuje, je `GENESIS_LOG` — úroveň logovania s funkčným predvolením, teda ladenie, nie funkcia. Vystavená ako možnosť app nie je a tento release ju nepridáva.

### Čo bolo v obraze a nedalo sa zapnúť

Tri veci boli v `main` aj v obraze od `0.1.3`, ale `run.sh` im nikdy nenastavil premennú, takže na jednotke boli tmavé. Vydať `0.1.4` len zvýšením čísla by znamenalo tvrdiť, že ELYSIUM-346, 348 a 355 sú v balíku, keď sa k nim prevádzkovateľ nemal ako dostať:

| Funkcia | Chýbala premenná | Čo to na jednotke znamenalo |
| --- | --- | --- |
| Záloha (ELYSIUM-348) | `GENESIS_BACKUP_DIR` | `POST /v1/backup` odpovedal 503 |
| Citlivé zariadenia pri hlase (ELYSIUM-346) | `GENESIS_SENSITIVE_DEVICES` | žiadne zariadenie nebolo citlivé, potvrdenie sa nikdy nevyžiadalo |
| Podpísaný kanál z Behavior (ELYSIUM-355) | `GENESIS_BEHAVIOR_KEY_ID`, `GENESIS_BEHAVIOR_SECRET`, `GENESIS_CENTRAL_UNIT_ID` | `POST /v1/behavior/decisions` vždy odpovedal `behavior_channel_not_configured` |

Nenastavené možnosti sa **neexportujú ako prázdne**. Core tieto premenné číta cez `env::var(...).ok()`, takže prázdny reťazec preň nie je „nenastavené", ale nastavená prázdna hodnota: prázdny `GENESIS_BEHAVIOR_KEY_ID` by zhodil štart app a prázdny `GENESIS_CENTRAL_UNIT_ID` by odmietol každé rozhodnutie Behavior. CI to overuje dvoma kontajnermi — jedným s prázdnymi možnosťami, ktorý musí naštartovať, a jedným s nastaveným kanálom, kde sa odmietnutie už nesmie odvolávať na chýbajúcu konfiguráciu a tajomstvo sa nesmie objaviť v logu.

## Kroky inštalácie

Celý postup vrátane aktualizácie a rollbacku je v [docs/ELYSIUM-352-instalacia.md](../docs/ELYSIUM-352-instalacia.md). Skrátene:

1. V Home Assistant app obchode pridať URL repozitára `https://github.com/Elysium-a-s/OS-Genesis` (musí byť pre Supervisor dostupný).
2. Nainštalovať **OS Genesis Pilot** a v nastaveniach vložiť dva rôzne náhodné tokeny `read_token` a `write_token`, každý s aspoň 32 znakmi. `member_token` a `guest_token` môžu zostať prázdne.
3. Spustiť app a kliknúť na **OPEN WEB UI** z lokálneho alebo vzdialeného HA. Panel automaticky použije adresu Ingressu. V paneli zadať Genesis `write_token` a načítať inventár. V logoch skontrolovať `Home Assistant inventory loaded` po pripojení aspoň jedného svetla/zásuvky.
4. Reštartovať HA Green a overiť automatický štart, zachovanie databázy a aktuálny inventár.
5. Zaznamenať CPU/RAM a logy pred reštartom a po ňom; tajomstvá z logov nezdieľať.

Pôvodný HTTP port 8765 sa vo verzii `0.1.2` nepublikuje. Pri aktualizácii HA app sa externé presmerovanie portu musí odstrániť, ak ho niekto nastavil ručne.
