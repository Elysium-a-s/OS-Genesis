# Home Assistant app pre Genesis pilot

`ha-app/genesis/` obsahuje konfiguráciu a Dockerfile pre Home Assistant Green (`aarch64`). Obraz obsahuje predkompilovaný Rust core; na Green sa Rust ani Cargo neinštalujú. CI vytvára ARM64 image archive a skúša štart, health endpoint, chránený inventár, reštart a perzistenciu SQLite v emulovanom ARM64 kontajneri.

Home Assistant app používa `homeassistant_api: true` a interný WebSocket proxy `ws://supervisor/core/websocket`. Supervisor poskytne `SUPERVISOR_TOKEN` v prostredí; štartovací skript ho odovzdá core ako `GENESIS_HA_TOKEN`. Tento token sa neukladá do Gitu ani do možností app. Používateľ nastaví samostatné `read_token` a `write_token` (každý najmenej 32 znakov). Write token povoľuje pilotné povely a nesmie byť vystavený verejnému internetu. SQLite sa ukladá do perzistentného `/data/genesis-ledger.sqlite3`.

## Stav distribúcie

Workflow [Build Genesis HA app (ARM64)](../.github/workflows/ha-app.yml) vytvorí testovaný `genesis-ha-pilot-arm64` artifact. `config.yaml` je pripravený pre obraz `ghcr.io/elysium-a-s/os-genesis-pilot:0.1.0`, ale workflow ho zatiaľ **nepublikuje do registra**. Preto zatiaľ nie je možné pridať repozitár a nainštalovať Genesis jedným kliknutím cez Home Assistant obchod. Pred pilotnou inštaláciou treba publikovať dostupný obraz verzie 0.1.0 a overiť prístup Home Assistantu k repozitáru/registru.

Obraz je testovaný len v GitHub CI emulácii. Reálny Home Assistant Green, obnova po reštarte HA OS, skutočné zariadenie a merania CPU/RAM na hardvéri sú otvorené akceptačné kroky Jira ELYSIUM-338 a ELYSIUM-336.

## Po publikovaní obrazu

1. V Home Assistant app obchode pridať URL repozitára `https://github.com/Elysium-a-s/OS-Genesis` (musí byť pre Supervisor dostupný).
2. Nainštalovať **OS Genesis Pilot** a v nastaveniach vložiť dva rôzne náhodné tokeny `read_token` a `write_token`, každý s aspoň 32 znakmi.
3. Spustiť app a otvoriť Health odkaz. V logoch skontrolovať `Home Assistant inventory loaded` po pripojení aspoň jedného svetla/zásuvky.
4. Reštartovať HA Green a overiť automatický štart, zachovanie databázy a aktuálny inventár.
5. Zaznamenať CPU/RAM a logy pred reštartom a po ňom; tajomstvá z logov nezdieľať.

HTTP port 8765 je určený iba pre dôveryhodnú domácu sieť. Nepresmerovávajte ho na verejný internet.
