# ELYSIUM-332 — inventár pilotu Home Assistant Green

**Stav:** čiastočne overené, čaká sa na údaje z používateľovej inštalácie a výber fyzického zariadenia.  
**Dátum:** 2026-09-29  
**Vlastník:** OS Genesis  
**Jira:** https://sarockylukas.atlassian.net/browse/ELYSIUM-332

## 1. Identita pilotného hardvéru

Používateľ potvrdil **Home Assistant Green**. Nižšie sú výrobné parametre modelu; netvrdia, že boli odmerané na konkrétnej jednotke.

| Položka | Hodnota | Dôkaz / stav |
| --- | --- | --- |
| Produkt | Home Assistant Green | Potvrdené používateľom |
| Modelové označenie výrobcu | NC-GREEN-1175 | Špecifikácia výrobcu; štítok konkrétnej jednotky neoverený |
| SoC | Rockchip RK3566 | Špecifikácia výrobcu |
| CPU | 4 × Arm Cortex-A55, 1,8 GHz | Špecifikácia výrobcu |
| Architektúra pre HA app | `aarch64` / ARM64 | Odvodené z CPU a dokumentácie HA app; potvrdiť v System information |
| RAM | 4 GB LPDDR4X | Špecifikácia výrobcu; voľná RAM konkrétnej jednotky neznáma |
| Úložisko | 32 GB eMMC | Špecifikácia výrobcu; voľné miesto konkrétnej jednotky neznáme |
| Sieť | Gigabit Ethernet | Špecifikácia výrobcu; konfigurácia siete konkrétnej jednotky neznáma |
| USB | 2 × USB 2.0 Type-A | Špecifikácia výrobcu; pripojené rádiá/dongle neznáme |
| Inštalačný typ | Home Assistant OS | Potvrdené používateľovým screenshotom: OS 18.3 |
| HA Core verzia | **2026.9.4** | Potvrdené používateľovým screenshotom z 2026-09-29 |
| HA OS verzia | **18.3** | Potvrdené používateľovým screenshotom z 2026-09-29 |
| Supervisor verzia | **Neoverené** | Doplniť zo System information |
| Voľná RAM / úložisko | **Neoverené** | Zmerať pred nasadením a po spustení Genesis |

Zdroj parametrov: [Home Assistant Green](https://www.home-assistant.io/green). Predinštalovaný HA OS: [typy inštalácie Home Assistant](https://www.home-assistant.io/installation/). Architektúry HA apps: [konfigurácia Home Assistant apps](https://developers.home-assistant.io/docs/apps/configuration/).

## 2. Prvé fyzické zariadenie

**Plánované zariadenie: Wi‑Fi žiarovka ovládaná cez Smart Life.** Používateľ uviedol, že ešte nie je pripojená. Presný výrobca/model nie je známy, preto nie je možné deklarovať fyzickú podporu alebo výsledok príkazu.

| Povinný údaj | Hodnota |
| --- | --- |
| Kategória (svetlo alebo zásuvka) | Wi‑Fi žiarovka, plánovaná |
| Výrobca a presný model | Čaká na výber |
| HA integrácia / entity ID | Plánovaná oficiálna integrácia Tuya pre Smart Life; potvrdiť po pripojení; entity ID zatiaľ neexistuje |
| Cesta spojenia (Wi‑Fi, Ethernet, Zigbee, Matter, hub/cloud) | Žiarovka cez Wi‑Fi do Smart Life; Home Assistant cez Tuya integráciu (cloud push podľa HA dokumentácie); skutočná cesta po spárovaní neoverená |
| Schopnosti pre pilot | Čítanie stavu; bezpečné zapnutie a vypnutie |
| Potvrdenie výsledku | Zistiť, či HA poskytne čerstvý stav po povele; provider odpoveď sama osebe nie je fyzické potvrdenie |
| Fyzický dôkaz | Test vykonať na skutočnom zariadení, nie iba v simulátore |

Výberové kritériá: jednoduchý bezpečný povel on/off, čitateľný stav, žiadna bezpečnostne kritická funkcia. Po pripojení do Smart Life sa v HA nastaví a overí oficiálna integrácia [Tuya](https://www.home-assistant.io/integrations/tuya/). Táto integrácia pracuje s účtom Smart Life a je klasifikovaná ako cloud push; Wi‑Fi pri žiarovke preto samo osebe neznamená lokálne vykonávanie povelov. Presný model, entity a cesta sa zapíšu pred implementáciou adaptéra.

## 3. Obmedzenia pre Genesis na Green

- Genesis prvýkrát nasadiť ako **Home Assistant app** v izolovanom kontajneri na `aarch64`; tým sa neprepisuje HA OS.
- Pred buildom potvrdiť v HA **Settings → System → About → System information** architektúru, typ inštalácie a verzie. [Oficiálny popis System information](https://www.home-assistant.io/more-info/system-information/).
- Vývoj a build vykonávať mimo Green; na Green skúšať hotový kontajner. Tento postup chráni obmedzené CPU, RAM a eMMC pred zbytočnou záťažou počas vývoja.
- Pripraviť prevádzkové meranie CPU, voľnej RAM, úložiska a odozvy pri štarte, idle a pri 20 po sebe idúcich poveloch; limity schváliť podľa nameranej rezervy.
- Hlasové STT/TTS/LLM **nezaraďovať na Green ako potvrdenú schopnosť** bez samostatného benchmarku. Ukladať čas odpovede, spotrebu RAM/CPU a vplyv na HA automatizácie. Ak výkon nepostačí, hlasová vrstva môže bežať mimo Green, zatiaľ čo core a zariadenia ostanú lokálne.
- Green nemá integrované Zigbee/Thread rádio; pri takom pilotnom zariadení overiť existujúci adaptér alebo border router. Výrobca uvádza USB rozšírenie a podmienky pre Matter over Thread. [Home Assistant Green](https://www.home-assistant.io/green).
- Nepovoľovať privilegovaný prístup ku hostiteľovi ani porty bez konkrétnej potreby a bezpečnostného posúdenia. HA app má vlastnú konfiguráciu a úložisko. [Konfigurácia HA apps](https://developers.home-assistant.io/docs/apps/configuration/).

## 4. Zber údajov bez prístupových údajov

Požiadať vlastníka o hodnoty **Installation type, Core, Supervisor, Operating System, Architecture** zo stránky System information a o značku/model prvého svetla alebo zásuvky. Nesnímať ani nezverejňovať token, heslo, verejnú IP adresu, sériové číslo, presnú polohu alebo celé diagnostické logy. Pre implementačný PR stačia verzie a modely; konkrétny entity ID môže zostať v lokálnej testovacej konfigurácii.

## 5. Brána dokončenia ELYSIUM-332

- [x] Model Green potvrdený používateľom.
- [x] Výrobné CPU, RAM, eMMC a spôsob štandardnej inštalácie zdokumentované so zdrojom.
- [x] Obmedzenia pre HA app a lokálny hlas zapísané ako návrh merania.
- [x] Skutočný typ inštalácie a verzie HA Core/OS potvrdené screenshotom z jednotky.
- [ ] Architektúra a verzia Supervisor potvrdené zo System information.
- [x] Zvolená kategória a plánovaná cesta: Wi‑Fi žiarovka.
- [x] Plánovaná aplikácia Smart Life a zodpovedajúca HA Tuya integrácia zdokumentované.
- [ ] Presný model, skutočná HA integrácia a fyzické pripojenie potvrdené.
- [ ] Priestor a voľná RAM zmerané alebo označené ako riziko pred nasadením.

Story môže ísť do **In Review** po doplnení údajov z konkrétnej jednotky a výbere zariadenia; do **Done** až po schválení inventára. Následný výkonový a fyzický test patrí implementačným stories ELYSIUM-334, ELYSIUM-336 a ELYSIUM-338.
