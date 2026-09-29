# Home Assistant adaptér

Implementácia prvého adaptéra je v [core/src/ha.rs](../../core/src/ha.rs), aby mohla bežať v tej istej Rust službe ako Genesis core. Pripája sa na oficiálne HA WebSocket API na `/api/websocket`: čaká na `auth_required`, odošle token, prihlási sa na `state_changed` a načíta snapshot cez `get_states`. Udalosti prijaté pred dokončením snapshotu aplikuje až po ňom.

Adaptér mapuje `light.*` a `switch.*` na Genesis `power` capability. `on` a `off` sú čerstvé hodnoty; `unavailable` ani neznámy stav netvrdia zapnutie či vypnutie. Po odpojení označí inventár ako `unknown`, po opätovnom pripojení sa znova autentifikuje, prihlási na udalosti a načíta celý snapshot. Zatiaľ neodosiela povely.

## Konfigurácia

- `GENESIS_HA_WS_URL`: napríklad `ws://homeassistant.local:8123/api/websocket` v dôveryhodnej domácej sieti. Pri vzdialenom spojení používajte `wss://`.
- `GENESIS_HA_TOKEN`: Home Assistant access token dodaný ako tajomstvo prostredia, nie v Gite ani v argumentoch príkazového riadku.
- `GENESIS_READ_TOKEN`: samostatný náhodný token s aspoň 32 znakmi pre `GET /v1/devices`. Endpoint bez neho vracia 503; s neplatným tokenom 401.

Nastavte obe HA premenné spolu. Logy obsahujú iba kategóriu chyby a počet načítaných zariadení; neobsahujú token, URL ani surové WebSocket rámce. Predvolené HTTP bindovanie je iba na `127.0.0.1`. Vystavenie mimo zariadenia vyžaduje ďalší návrh autentifikácie a bezpečnej siete.

## Stav pilotu

Lokálne testy používajú simulovaný WebSocket server vrátane dvoch spojení. Skutočná žiarovka alebo zásuvka zatiaľ nie je pripojená do Home Assistantu; načítanie reálneho zariadenia sa ešte musí overiť na Home Assistant Green.
