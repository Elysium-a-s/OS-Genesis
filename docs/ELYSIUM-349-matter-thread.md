# ELYSIUM-349 — Matter a Thread na pilotnom hardvéri (spike)

**Stav:** spike uzavretý rozhodnutím **odložiť natívnu implementáciu**. Podpora Matteru sa v tomto dokumente **nedeklaruje** — fyzický test ešte neprebehol.  
**Dátum:** 2026-09-30  
**Vlastník:** OS Genesis  
**Jira:** https://sarockylukas.atlassian.net/browse/ELYSIUM-349

Nadväzuje na [ELYSIUM-332 — inventár pilotu](ELYSIUM-332-pilot-hardware.md), ktorý už uvádza, že Green nemá integrované 802.15.4 rádio. Tento spike to rozpisuje do dôsledkov a dáva rozhodnutie.

## 1. Rozhodnutie

**Natívny Matter v Genesis sa odkladá.** Genesis si nebude robiť vlastný Matter controller ani vlastnú fabric.

Dôvod v jednej vete: **Matter zariadenie sa ku Genesis dostane už dnes a bez jediného riadku Matter kódu** — Home Assistant je controller, Matter zariadenia sa v ňom objavia ako obyčajné entity a náš adaptér ich preberie ako každú inú `light.*` alebo `switch.*`. Natívna implementácia by túto cestu nezlepšila, ale zdvojila.

Rozhodnutie nie je „Matter nepodporujeme". Je to „Matter podporujeme cez Home Assistant a nestaviame druhú cestu". Podmienky, ktoré by rozhodnutie otočili, sú v časti 9.

## 2. Rádiový hardvér pilotu

| Položka | Hodnota | Zdroj / stav |
| --- | --- | --- |
| 802.15.4 rádio (Zigbee/Thread) | **Žiadne** | Špecifikácia Green neuvádza rádio; výrobca odporúča doplniť Connect ZBT-2 |
| Wi-Fi | **Neuvedené v špecifikácii** | Pod „Networking" je iba Gigabit Ethernet |
| Bluetooth | **Neuvedené v špecifikácii** | FAQ výrobcu uvádza Bluetooth ako vec USB doplnku, nie zabudovanú |
| Ethernet | Gigabit | Špecifikácia výrobcu |
| USB | 2 × USB 2.0 Type-A, 5 V do **2 A dohromady** | Špecifikácia výrobcu |
| SoC / CPU / RAM | RK3566, 4 × Cortex-A55 1,8 GHz, 4 GB LPDDR4X | Špecifikácia výrobcu |

Zdroj: [Home Assistant Green](https://www.home-assistant.io/green) (sekcia Hardware specifications a FAQ).

Tri dôsledky, ktoré treba čítať ako obmedzenia, nie ako poznámky:

1. **Bez dongle Green nevie ani Zigbee, ani Thread.** Akékoľvek Matter-over-Thread zariadenie potrebuje border router — buď ZBT-2 v Green, alebo cudzí (Apple, Google).
2. **Dva USB porty a 2 A dohromady je celý rozpočet.** Zigbee dongle a Z-Wave dongle naraz už porty vyčerpajú a napájanie je spoločné. Dokumentácia Thread navyše k adaptéru odporúča **USB predlžovací kábel** (odstup od zdroja rušenia).
3. **4 GB RAM zdieľa všetko.** Home Assistant, Matter Server app, OTBR app, Genesis a prípadný hlas. Matter Server je dnes Node.js služba (časť 8). Jej spotrebu na Green **nikto neodmeral** — patrí to k meraniam z ELYSIUM-332.

## 3. Čo z Matteru funguje dnes, bez zásahu do Genesis

Home Assistant **je** Matter controller. Pre komunikáciu so zariadeniami spúšťa vlastný controller ako app (Matter Server) a integrácia sa na ňu pripája cez WebSocket. Zariadenia sa potom objavia ako bežné entity HA.

Náš adaptér ([core/src/ha.rs](../core/src/ha.rs)) nerieši, akým protokolom entita prišla. Mapuje `light.*` a `switch.*` na capability `power`. Matter žiarovka je pre Genesis `light.nieco` — a tým prechádza celou existujúcou vrstvou: ledger, idempotencia, granty, hlas, audit. **Nič z toho netreba pre Matter meniť.**

Praktický dôsledok: ak sa na pilote objaví Matter žiarovka cez Wi-Fi, Genesis ju ovláda tou istou cestou ako Tuya žiarovku z ELYSIUM-332. Jediné, čo Genesis nevie, je ju **pridať** (časť 5).

Verzie na pilotnej jednotke (HA Core 2026.9.4, HA OS 18.3 podľa ELYSIUM-332) sú nad všetkými minimami: Matter Server app žiada Core ≥ 2024.6.0, OTBR app ≥ 2025.7.0 a Matter over Thread žiada HA OS ≥ 10.

Zdroje: [integrácia Matter](https://www.home-assistant.io/integrations/matter/), [`matter_server/config.yaml`](https://github.com/home-assistant/addons/blob/master/matter_server/config.yaml), [`openthread_border_router/config.yaml`](https://github.com/home-assistant/addons/blob/master/openthread_border_router/config.yaml).

## 4. Čo Matter over Thread vyžaduje navyše

Thread je iba prenos — sám nič neovláda; ovládanie robí Matter alebo HomeKit nad ním. Zariadenie v mesh sieti potrebuje **border router**, ktorý premosťuje Thread a LAN.

Stav podľa dokumentácie výrobcu, citovaný presne, pretože je to najdôležitejšia veta celého spike:

> Out of the box, Home Assistant Connect ZBT-1, Connect ZBT-2, and Yellow run Zigbee, not Thread. Currently, enabling Thread involves manual steps. The integration of the Home Assistant based Thread border router with Matter is work-in-progress.
> — [integrácia Thread](https://www.home-assistant.io/integrations/thread/)

Produktová stránka Green k ZBT-2 pripája hviezdičku „Thread support is currently under development".

Čiže: **Thread cesta na Green je dnes otvorená stavba**, nie hotová funkcia. Konkrétne to znamená:

- ZBT-2 treba prepnúť na OpenThread firmware manuálnym postupom; z výroby je to Zigbee.
- Jedno rádio robí jednu vec. Kto chce Zigbee **aj** Thread, potrebuje dve rádiá alebo multiprotocol adaptér, a pri multiprotocole musia byť siete na rovnakom kanáli.
- Alternatíva bez dongle je **cudzí border router** — Apple (HomePod mini/2. gen, Apple TV 4K 2./3. gen), Google (Nest Hub 2. gen, Nest Hub Max, Nest Wifi Pro, Google TV Streamer). Prevádzka cez cudzí TBR je dokumentovaná ako funkčná a obsah paketov TBR nevidí.
- Ale: **OTA aktualizácie Matter zariadení cez Apple border router nefungujú.** Ak má Genesis niekedy tvrdiť, že drží zariadenia aktualizované, Apple-only TBR to znemožní.

## 5. Párovanie je tvrdý limit, nie nepríjemnosť

Toto je jediné zistenie, ktoré mení architektonické plány, preto stojí samostatne.

Matter zariadenie sa páruje **telefónom**. Controller HA na to používa Companion app: cez Bluetooth telefónu sa zariadeniu pošlú sieťové prístupové údaje, a až potom zariadenie komunikuje po Wi-Fi alebo Thread. Dokumentácia to ohraničuje výslovne:

- Pridanie Matter zariadenia **funguje iba v Companion app**, nie v prehliadači.
- **Mac app Matter zariadenia pridať nevie**; treba iPhone alebo iPad (iOS/iPadOS ≥ 16.4) alebo Android ≥ 8.1 (odporúčané 12+) s Google Play službami.
- Server síce môže mať vlastný Bluetooth adaptér, ale **HA ho na párovanie zámerne nepoužíva**.
- Bez QR kódu alebo číselného setup kódu sa zariadenie spárovať **nedá** — ani po factory resete.

Dôsledok pre nás: **Genesis panel Matter zariadenie nikdy nespáruje.** Ani keby sme Matter implementovali natívne, onboarding by ostal na telefóne s BLE. Genesis vie Matter zariadenie *používať*; *prijať do domácnosti* ho vie Home Assistant s telefónom.

Pri Thread zariadení navyše telefón musí poznať prístupové údaje Thread siete (pri HA border routeri sa synchronizujú z HA), inak párovanie skončí chybou „this device requires a border router".

## 6. Sieťové nároky

Matter nie je vlastný rádiový protokol — je to aplikačný protokol nad IP, ktorý stojí a padá na **IPv6 a multicaste**. Z dokumentácie Matter serveru a integrácie:

| Požiadavka | Prečo |
| --- | --- |
| IPv6 zapnuté na HA (Settings → System → Network) | Bez toho Matter nekomunikuje. Verejná IPv6 konektivita ani DHCPv6 nie sú potrebné. |
| mDNS multicast musí prejsť voľne | Objavovanie zariadení ide cez mDNS. |
| **Žiadne** mDNS reflektory / Avahi forwardery medzi segmentmi | Podľa dokumentácie kazia alebo výrazne brzdia Matter pakety. |
| `net.ipv6.conf.all.forwarding = 0`, `accept_ra` zapnuté, `accept_ra_rt_info_max_plen=64` | Pri zapnutom forwardingu jadro nerieši reachability a Thread trasy sa pokazia. |
| `nf_conntrack_udp_timeout_stream` ≥ 1800 (odporúčané 3600) na jadre, ktoré filtruje | Predvolených 120 s zahodí hlásenia batériových („sleepy") zariadení, ktoré sa ozývajú v minútach. |
| Nefiltrovať podľa portu 5540 | Špecifikácia jediný port nevyžaduje; zariadenie si port ohlási cez mDNS. Filtrovať podľa rozhrania alebo IPv6 prefixu. |
| Plochá sieť; pozor na VLAN, multicast filtering, IGMP snooping | Matter je navrhnutý pre bežnú domácu sieť, nie pre enterprise topológiu. |

HA OS 10+ toto má správne nastavené z výroby vrátane spracovania ICMPv6 Router Advertisements a conntrack nastavení — **to je hlavný dôvod, prečo je HA OS podporovaná cesta a vlastný kontajner nie.**

Zdroje: [os_requirements.md](https://github.com/matter-js/matterjs-server/blob/main/docs/os_requirements.md), [docker.md](https://github.com/matter-js/matterjs-server/blob/main/docs/docker.md), [integrácia Matter](https://www.home-assistant.io/integrations/matter/).

## 7. Limity HA app — čo by natívny Matter stál

Genesis dnes beží ako HA app s jedným statickým Rust binárom, bez prístupu k hostiteľovi. Pre porovnanie, čo si oficiálne apps nechávajú povoliť (z ich `config.yaml`):

| App | Čo žiada |
| --- | --- |
| **Matter Server** | `host_network: true`, `host_dbus: true`, `hassio_api`, `ingress`, port 5580, voliteľný `bluetooth_adapter_id` alebo `ble_proxy`, `arch: aarch64, amd64`, `homeassistant: 2024.6.0` |
| **OpenThread Border Router** | `host_network: true`, `host_uts: true`, `gpio: true`, `privileged: [IPC_LOCK, NET_ADMIN]`, `devices: [/dev/net/tun]`, sériové zariadenie `device(subsystem=tty)`, porty 8080/8081, `homeassistant: 2025.7.0` |

Obe sú `aarch64`, takže na Green by architektúra nebola problém. Problém je rozsah oprávnení: natívny Matter controller v Genesis app by potreboval **host networking, IPv6 ND, vlastný mDNS a BLE** — teda presne to, čo ELYSIUM-332 zakazuje brať bez konkrétnej potreby a bezpečnostného posúdenia. A dostal by za to funkciu, ktorú už máme cez HA.

K tomu vecná poznámka k modelu tajomstiev z ELYSIUM-347: Genesis si dnes ukladá **iba odtlačky** prístupových údajov. Matter fabric sa takto uložiť nedá — kľúče fabric musia zostať použiteľné, aby controller vedel so zariadeniami hovoriť. Natívny Matter teda nie je len „viac kódu", je to **zmena bezpečnostného modelu**: Genesis by sa stal držiteľom použiteľných kryptografických údajov k zariadeniam. To si zaslúži vlastné rozhodnutie, nie vedľajší efekt.

## 8. Možnosti SDK

| SDK | Jazyk | Licencia | Verzia Matteru | Stav pre nás |
| --- | --- | --- | --- | --- |
| [matterjs-server](https://github.com/matter-js/matterjs-server) (Open Home Foundation) | JavaScript / Node.js | Apache-2.0 | 1.6.0 | To, čo HA app dnes spúšťa. WebSocket API, drop-in za Python Matter Server. **Beta; podľa vlastného README ešte nie je znovu certifikovaný CSA.** |
| [python-matter-server](https://github.com/matter-js/python-matter-server) | Python | Apache-2.0 | — | Predchodca nad oficiálnym CHIP SDK. **Officially certified** software component pre Matter controller — ale **verzia 8.1.2 je posledná**, bez ďalších aktualizácií a podpory. |
| [connectedhomeip](https://github.com/project-chip/connectedhomeip) | C++ | Apache-2.0 | — | Referenčná implementácia CSA, základ certifikovaného Python serveru. Ťažký build, do nášho Rust binára sa nehodí. |
| [rs-matter](https://github.com/project-chip/rs-matter) | **Rust** | Apache-2.0 | 1.6 | Jediná cesta, ktorá by sedela do našej služby. Zvláda aj controller a commissioner. Ale: **API nie je stabilné** a certifikácia produktu nad ním je v roadmape ako *ďalší krok*, teda hotová nie je. |

Licenčne je to čisté — všetky štyri sú Apache-2.0, žiadne poplatky za samotný kód.

**Certifikácia je iná vec a práve teraz je v prechodnom stave.** Toto sú citovateľné skutočnosti, nie dohady:

- Matter controller **certifikácii podlieha** a certifikovať sa dá aj ako softvérový komponent, nie len ako hotové zariadenie: Python Matter Server je [officially certified](https://csa-iot.org/csa_product/open-home-foundation-matter-server/) software component na vytvorenie Matter controllera.
- Ten certifikovaný komponent je ale **na konci života**. Verzia 8.1.2 je posledná; ďalšie aktualizácie ani podporu nedostane a odporúča sa prejsť na matter.js server.
- Nástupca, ktorý HA app dnes spúšťa, **certifikovaný (ešte) nie je** — jeho README to uvádza sám: „not yet officially re-certified by the CSA, but will be in the future".
- Certifikáciu zastrešil **člen CSA** — Python server financovala Nabu Casa, ktorá je členom CSA, a komponent darovala Open Home Foundation.

Praktický dôsledok pre Elysium: **certifikovaná cesta k Matter controlleru dnes vedie cez cudziu, doživanú komponentu a jej necertifikovaného nástupcu.** To nie je dôvod niečo robiť inak — pre nás sa nemení nič, kým Matter beží v Home Assistante a nie u nás — ale je to dôvod **netvrdiť o Genesis nič o Matter certifikácii**.

Čo by Elysium muselo urobiť, keby sme Matter niekedy implementovali sami — členstvo v CSA, produkčné VID/PID, zápis do Distributed Compliance Ledger (Matter app má voľbu `enable_test_net_dcl` práve pre testovací DCL), certifikačné testy — **sme z tohto prostredia neoverili**, lebo `csa-iot.org` odtiaľ nie je dostupný. Vyššie uvedené štyri body sú z README repozitárov; rozsah a cena certifikácie nie. To treba overiť priamo s CSA, a skôr, než by sa o natívnom Matteri v Elysiu čokoľvek oznámilo verejne.

## 9. Kedy sa toto rozhodnutie prehodnotí

Odloženie platí, kým nenastane niektorá z týchto vecí. Sú napísané tak, aby boli rozhodnuteľné, nie ako pocit:

1. **Genesis má bežať bez Home Assistanta.** Vtedy padá celý dôvod odloženia, lebo Matter zariadenia by inak nemal odkiaľ dostať.
2. **HA Matter cesta sa zlomí alebo skončí.** Napríklad ak Matter Server prestane byť podporovaný na HA OS pre `aarch64`.
3. **Pilotné zariadenie je Matter-only v doméne, ktorú HA neposkytne použiteľne.** Dnes to nie je prípad — pilot je `light`/`switch`.
4. **Potrebujeme silnejší dôkaz o stave zariadenia, než HA vie dať.** Z tejto päťky je to najvecnejší dôvod: náš ledger odlišuje `provider_confirmed` od `device_confirmed` a `device_confirmed` sa dnes opiera o čerstvý stav z HA. Matter subscription na atribút by dala priamejší dôkaz. Ak sa zosúlaďovanie (ELYSIUM-344) ukáže ako nedostatočne presné práve preto, že medzi Genesis a zariadením stojí HA, je to legitímny dôvod otvoriť natívnu cestu.
5. **rs-matter stabilizuje API a existuje certifikovateľná cesta pre controller.** Bez oboch je implementácia stavba na piesku.

Aj po otočení rozhodnutia by párovanie zostalo na telefóne (časť 5). To sa nezmení ničím, čo môžeme naprogramovať.

## 10. Čo treba fyzicky otestovať, než sa podpora vyhlási

Akceptačné kritérium tiketu žiada **nedeklarovať podporu pred fyzickým testom**. Tento dokument ju preto nedeklaruje. Toto sú dva testy, ktoré ju môžu vyhlásiť, s tým, čo sa počíta ako dôkaz.

### Test A — Matter cez Wi-Fi/Ethernet (bez nového rádia)

Potrebné: jedno **Matter certifikované** Wi-Fi svetlo alebo zásuvka (hľadať logo Matter, nie Thread — Thread logo podporu Matteru nezaručuje), jeho QR alebo setup kód, iPhone/iPad s iOS ≥ 16.4 alebo Android ≥ 8.1 s Companion app a zapnutým Bluetooth.

1. V HA pridať integráciu Matter, nechať nainštalovať Matter Server app; overiť IPv6 na **Automatic** alebo static.
2. Spárovať zariadenie Companion app na telefóne.
3. Zapísať `entity_id` a doménu entity, ktorá v HA vznikla.
4. Overiť, že Genesis inventár zariadenie vidí ako `power` capability — `GET /v1/devices`.
5. Poslať `POST /v1/commands` na zapnutie a vypnutie a **fyzicky pozrieť na žiarovku**.
6. Overiť, že ledger dal `device_confirmed`, nie iba `provider_confirmed`.
7. Odmerať RAM a CPU Green pred a po pridaní Matter Server app.

Dôkaz: entity ID a doména, dva povely s fyzicky overeným výsledkom, stav povelu z ledgeru, čísla RAM/CPU. Odpoveď providera sama osebe dôkazom nie je — to je pravidlo z ELYSIUM-332 a platí aj tu.

### Test B — Matter cez Thread

Potrebné: Connect ZBT-2 a USB predlžovací kábel (alebo cudzí border router), Matter-over-Thread zariadenie.

1. Podľa postupu výrobcu prepnúť ZBT-2 na OpenThread a nainštalovať OTBR app; zapísať, čo si postup vyžiadal a čo z neho nefungovalo.
2. V Settings → Connectivity → Thread overiť, že border router vznikol, a synchronizovať prístupové údaje do telefónu.
3. Spárovať Thread zariadenie a zopakovať kroky 3–6 z testu A.
4. Nechať batériové zariadenie hlásiť stav aspoň jeden celý interval a overiť, že hlásenie nezmizlo (to je ten conntrack timeout z časti 6).
5. Zapísať, či OTA aktualizácia zariadenia z HA prešla.

Dôkaz: to isté ako v teste A, plus výsledok kroku 4 a 5.

Kým test A neprejde, **veta „Genesis podporuje Matter" sa nemá objaviť nikde** — ani v README, ani v Jire, ani v komunikácii o produkte.

## 11. Čo tento spike netvrdí

- Netvrdí, že Matter zariadenie bolo s Genesis odskúšané. **Žiadne nebolo.**
- Netvrdí, že Thread na Green funguje. Podľa výrobcu je to postup s manuálnymi krokmi a prepojenie HA border routera s Matterom je rozpracované.
- Netvrdí nič o spotrebe Matter Server app na Green. Neodmerané.
- Netvrdí, aký je rozsah a cena certifikácie controllera u CSA. To, že controller certifikácii podlieha, overené je; podmienky pre Elysium nie (časť 8).
- Netvrdí, že Green nemá Wi-Fi ani Bluetooth. Tvrdí, že **špecifikácia ich neuvádza** a výrobca ich spomína ako vec USB doplnku.

## 12. Brána dokončenia ELYSIUM-349

- [x] Rádiový hardvér pilotu zdokumentovaný so zdrojom, vrátane toho, čo Green nemá.
- [x] Podporované zariadenie: cesta pre Matter cez Wi-Fi/Ethernet aj cez Thread rozpísaná, s požiadavkou na logo Matter a na QR/setup kód.
- [x] Licenčné nároky zistené (štyri SDK, všetky Apache-2.0). Certifikácia: zistené, že controller jej podlieha a že certifikovaný komponent dnes končí život, kým jeho nástupca certifikovaný nie je; rozsah a cena pre Elysium **označené ako neoverené** s dôvodom.
- [x] Build nároky: architektúry apps, minimálne verzie HA, a čo by si natívny controller vyžiadal od oprávnení app.
- [x] Možnosti SDK porovnané vrátane Rust cesty a jej stavu.
- [x] Limity HA OS app vymenované z ich `config.yaml`, nie z dojmu.
- [x] Rozhodnutie **odložiť**, s dôvodmi a s piatimi rozhodnuteľnými podmienkami na prehodnotenie.
- [x] Podpora **nedeklarovaná**; fyzický test rozpísaný do dvoch scenárov s definovaným dôkazom.
