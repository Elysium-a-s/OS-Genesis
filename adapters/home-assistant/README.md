# Home Assistant adaptér

Implementácia prvého adaptéra je v [core/src/ha.rs](../../core/src/ha.rs), aby mohla bežať v tej istej Rust službe ako Genesis core. Pripája sa na oficiálne HA WebSocket API na `/api/websocket`: čaká na `auth_required`, odošle token, prihlási sa na `state_changed` a načíta snapshot cez `get_states`. Udalosti prijaté pred dokončením snapshotu aplikuje až po ňom.

Adaptér mapuje `light.*` a `switch.*` na Genesis `power` capability. `on` a `off` sú čerstvé hodnoty; `unavailable` ani neznámy stav netvrdia zapnutie či vypnutie. Po odpojení označí inventár ako `unknown`, po opätovnom pripojení sa znova autentifikuje, prihlási na udalosti a načíta celý snapshot. Povely odosiela cez `call_service`; vykonávanie je v [core/src/ha_command.rs](../../core/src/ha_command.rs).

## Miestnosti: ako sa mapuje HA area na entitu

Adaptér okrem stavov číta tri registre Home Assistanta a z nich skladá mapovanie miestností:

| Dotaz | Načo |
| --- | --- |
| `get_config` | `location_name`, teda názov domácnosti |
| `config/area_registry/list` | `area_id` a názov každej miestnosti |
| `config/device_registry/list` | miestnosť zariadenia, od ktorého ju entita môže dediť |
| `config/entity_registry/list` | miestnosť entity, a jej zariadenie |

**Prečo všetky tri registre.** Entita má miestnosť buď priamo (`area_id` v entity registri), alebo ju dedí od zariadenia, ku ktorému patrí. Dedenie je ten častejší prípad, pretože v Home Assistante človek priraďuje do miestnosti zariadenie, nie jednotlivé entity. Bez `device_registry` by takáto entita vyšla ako bez miestnosti. Priame priradenie vyhráva nad dedeným — je konkrétnejšie, je to výnimka nastavená práve pre tú entitu.

**Identifikátory.** Genesis pridá predponu providera, takže z `living_room` je `ha:living_room`, rovnako ako z `light.living` je `ha:light.living`. HA `area_id` je slug, ktorý sa pri **premenovaní miestnosti nemení** — preto je stabilný a panel si naň môže viazať výber.

**Čo sa do mapovania nedostane.** Miestnosť bez názvu (v paneli by bola prázdny riadok), entita v domény, ktorú Genesis neovláda, a priradenie do miestnosti, ktorú area register nepozná. Zariadenie potom vyjde ako bez miestnosti, čo je pravda o tom, čo o ňom vieme.

**Zmeny.** Adaptér je prihlásený na `area_registry_updated`, `device_registry_updated` a `entity_registry_updated`. Tie udalosti nenesú nový záznam, iba to, že sa register zmenil, takže Genesis na ne registre prečíta znova — presun entity do inej miestnosti aj premenovanie miestnosti tak idú jednou cestou. Mapovanie sa prepíše až keď dorazia všetky štyri odpovede; inak by medzi nimi existoval okamih, v ktorom panel vidí miestnosti bez zariadení.

**Premenovanie entity** naopak registrovou cestou nejde. Home Assistant prepíše `friendly_name` v stave, takže prichádza ako obyčajný `state_changed` a adaptér ho spracuje rovnako ako zmenu zapnutia.

**Token bez administrátorských práv.** `config/*_registry/list` vyžaduje administrátora. Keď zlyhá, sedenie sa **nezhodí**: inventár a povely fungujú ďalej a chýbajúce mapovanie sa prizná ako `rooms_incomplete` v `GET /v1/inventory`. Strata názvov miestností je neúmerne menšia než strata ovládania domácnosti — a panel to má povedať, nie tvrdiť, že domácnosť žiadne miestnosti nemá.

## Na akom protokole zariadenie beží

Adaptér to nerieši a nemá prečo. Rozhoduje doména entity, nie to, či zariadenie hovorí Wi-Fi, Zigbee, Z-Wave alebo Matterom — Home Assistant protokol skryje a Genesis dostane `light.*` alebo `switch.*`. Preto Matter zariadenie prechádza celou existujúcou vrstvou (ledger, granty, hlas, audit) bez akéhokoľvek Matter kódu v Genesis.

Čo z toho **nevyplýva**: že Genesis vie zariadenie spárovať. Matter párovanie vyžaduje telefón s Bluetooth a Home Assistant Companion app; panel Genesis na to cestu nemá. Podrobne, aj s rozhodnutím nestavať natívny Matter controller, je to v [docs/ELYSIUM-349-matter-thread.md](../../docs/ELYSIUM-349-matter-thread.md). Podpora Matteru zatiaľ **nie je deklarovaná** — fyzický test neprebehol.

## Konfigurácia

- `GENESIS_HA_WS_URL`: napríklad `ws://homeassistant.local:8123/api/websocket` v dôveryhodnej domácej sieti. Pri vzdialenom spojení používajte `wss://`.
- `GENESIS_HA_TOKEN`: Home Assistant access token dodaný ako tajomstvo prostredia, nie v Gite ani v argumentoch príkazového riadku.
- `GENESIS_READ_TOKEN`: samostatný náhodný token s aspoň 32 znakmi pre `GET /v1/devices`. Endpoint bez neho vracia 503; s neplatným tokenom 401.

Nastavte obe HA premenné spolu. Logy obsahujú iba kategóriu chyby a počet načítaných zariadení; neobsahujú token, URL ani surové WebSocket rámce. Predvolené HTTP bindovanie je iba na `127.0.0.1`. Vystavenie mimo zariadenia vyžaduje ďalší návrh autentifikácie a bezpečnej siete.

## Stav pilotu

Lokálne testy používajú simulovaný WebSocket server vrátane dvoch spojení, presunu entity do inej miestnosti, premenovania entity a registrov, ktoré sa nepodarilo prečítať. Skutočná žiarovka alebo zásuvka zatiaľ nie je pripojená do Home Assistantu; načítanie reálneho zariadenia a reálnych miestností sa ešte musí overiť na Home Assistant Green.
