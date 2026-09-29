# OS Genesis

OS Genesis je pripravovaný systém centrálnej jednotky a kontrolného centra Elysium. Prvý pilot bude bežať vedľa existujúceho Home Assistantu na jeho hardvéri. Home Assistant poskytne zariadenia a ich udalosti; Genesis vytvorí vlastný model zariadení, vykonávanie príkazov a rozhranie pre panely a Elysium Behavior.

> Stav: v1 kontrakt, Rust core s execution ledgerom a HA WebSocket adaptér pre svetlá a zásuvky. Fyzické overenie zariadenia, panel a inštalovateľný balík ešte nie sú hotové.

## Prvý overiteľný cieľ

Genesis načíta jedno skutočné svetlo z Home Assistantu, zobrazí jeho stav na iPade/web paneli, odošle príkaz a zaznamená rozdiel medzi odoslaním, potvrdením poskytovateľom a overeným stavom zariadenia. Až potom sa pripojí prvý časovo obmedzený scenár Elysium Behavior.

## Štruktúra

| Cesta | Zodpovednosť |
| --- | --- |
| `core/` | Rust služba: API, domácnosti, zariadenia, oprávnenia, príkazy, udalosti |
| `adapters/home-assistant/` | Prvý adaptér: discovery, stav, povely a spätná synchronizácia s HA |
| `panel/` | jeden Flutter/Dart projekt pre nástenný iPad kontrolný panel, iPhone a web; Linux panel podľa pilotu |
| `ha-app/` | Balenie Genesis ako Home Assistant app (predtým add-on) pre HA OS |
| `contracts/` | Verziovaná v1 JSON Schema pre domácnosť, zariadenie, schopnosť, pozorovanie a príkaz |
| `docs/` | Architektúra, rozhodnutia a prevádzkové návody |
| `tests/` | End-to-end scenáre a dôkazy z fyzických zariadení |

V `contracts/` je implementovaná a testovaná prvá verzia wire kontraktu. `core/` obsahuje spustiteľnú Rust službu s health endpointom a interným execution ledgerom. HA adaptér sa pripája cez WebSocket a poskytuje read-only inventár cez chránené API. Panel a HA app zatiaľ neobsahujú funkčný runtime.

## Hranice systému

- Genesis core beží na Linuxe; Rust je východiskový jazyk služby, nie vlastný kernel.
- Všetky nové Genesis používateľské rozhrania vrátane nástenného iPad kontrolného panelu sa vyvíjajú vo Flutteri/Darte. Flutter je klientské rozhranie, nie operačný systém ani ovládač rádiových protokolov.
- HA adaptér je prvá cesta k zariadeniam. Genesis nesmie vydávať prijatie povelu za potvrdenú fyzickú zmenu.
- Elysium Behavior rozhoduje o cieli a oprávnení; Genesis kontroluje vykonateľnosť, vykoná akciu a vráti pravdivý výsledok.
- Existujúca Elysium iOS aplikácia zostáva v Swifte a prepája sa s Genesis Flutter panelom cez API alebo deep link; FastAPI backend a PostgreSQL zostávajú vo vlastných repozitároch. Zmeny ich kontraktov sa robia v príslušných repozitároch.

## Poradie vývoja

1. Spísať `contracts/` pre inventár, stav a príkazy vrátane verzie schémy, identity domácnosti, idempotency key a korelačného ID.
2. Vytvoriť minimálny `core/` s health endpointom, bezpečnou konfiguráciou a interným execution ledgerom; adaptér následne poskytne dôkazy o výsledku príkazu.
3. Napísať `adapters/home-assistant/` pre jedno svetlo a otestovať čítanie stavu, príkaz, timeout a obnovu spojenia.
4. Vytvoriť `panel/` s jednou miestnosťou, zariadením a viditeľným stavom príkazu.
5. Zabaliť službu do `ha-app/` a overiť na konkrétnom HA hardvéri.
6. Pripojiť jeden scenár Elysium Behavior: oprávnenie, vykonanie, expirácia a bezpečné zosúladenie.

## Pravidlá pre prvý pilot

- Pred nasadením zaznamenať model HA hardvéru, architektúru CPU, RAM a spôsob inštalácie.
- Nepísať prihlasovacie údaje, tokeny ani osobné údaje do repozitára alebo logov.
- Rozlišovať `accepted`, `sent`, `provider_confirmed`, `device_confirmed`, `unknown` a `failed`; úroveň potvrdenia závisí od schopností adaptéra.
- Každý príkaz má identitu aktéra, autorizáciu, idempotency key a auditovateľný výsledok.
- Kritické akcie Elysium testovať aj po reštarte, výpadku siete a expirácii oprávnenia.
- Podporu zariadenia deklarovať až po fyzickom teste, nie iba po simulácii.

## Súvisiace repozitáre

- [Elysium FastAPI-module](https://github.com/Elysium-a-s/FastAPI-module) — Behavior rozhodnutia a serverové API.
- [Elysium PostgreSQL](https://github.com/Elysium-a-s/PostgreSQL) — databázové migrácie Elysium.
- [Elysium IOS](https://github.com/Elysium-a-s/IOS) — existujúca mobilná aplikácia.
- [hacs-elysium](https://github.com/Elysium-a-s/hacs-elysium) — existujúca integrácia Elysium v Home Assistante.

Produktové a analytické návrhy sú v Confluence v priečinku [OS Genesis](https://sarockylukas.atlassian.net/wiki/spaces/Elysium/folder/46891009/OS+Genesis).
