# Home Assistant app pre Genesis pilot

`ha-app/genesis/` obsahuje konfiguráciu a Dockerfile pre Home Assistant Green (`aarch64`). Obraz obsahuje predkompilovaný Rust core; na Green sa Rust ani Cargo neinštalujú. CI vytvára ARM64 image archive a skúša štart, health endpoint, chránený inventár, reštart a perzistenciu SQLite v emulovanom ARM64 kontajneri.

Home Assistant app používa `homeassistant_api: true` a interný WebSocket proxy `ws://supervisor/core/websocket`. Supervisor poskytne `SUPERVISOR_TOKEN` v prostredí; štartovací skript ho odovzdá core ako `GENESIS_HA_TOKEN`. Tento token sa neukladá do Gitu ani do možností app. Používateľ nastaví samostatné `read_token` a `write_token` (každý najmenej 32 znakov). Write token povoľuje pilotné povely a nesmie byť vystavený verejnému internetu. SQLite sa ukladá do perzistentného `/data/genesis-ledger.sqlite3`. Je to celý stav Genesis; záloha, obnova a rollback sú v [docs/ELYSIUM-348-obnova.md](../docs/ELYSIUM-348-obnova.md).

## Flutter panel v HA app

CI pred zostavením ARM64 obrazu zostaví a otestuje Flutter web panel. Obraz obsahuje statické súbory v `/usr/share/genesis-panel`; webové UI sa otvára na `http://<IP-Green>:8765/` a používa API na tej istej adrese. Watchdog ostáva na `/health`. Zadaný prístupový token zostáva v pamäti otvoreného panelu a neposiela sa v URL. Samotný CI smoke test nie je dôkazom behu na Green ani fyzickom iPade.

## Stav distribúcie

Workflow [Build Genesis HA app (ARM64)](../.github/workflows/ha-app.yml) obraz zostaví, spustí naň smoke test a uloží `genesis-ha-pilot-arm64` artifact. Pri pushi do `main` ho **publikuje do GHCR** — a publikuje presne ten obraz, ktorý smoke testom prešiel, nie druhý build.

Meno obrazu aj verziu si workflow číta z `genesis/config.yaml`, pretože Supervisor sťahuje presne `<image>:<version>`; keby to bolo napísané na dvoch miestach, rozišlo by sa to. Z tej istej verzie sa plní aj `io.hass.version` v obraze a CI overuje, že značka a label hovoria to isté.

Publikujú sa dve značky:

- `sha-<commit>` — nemenná, vzniká pri každom pushi do `main`. Slúži na dohľadanie, čo presne beží.
- `<version>` — vydanie. **Nikdy sa neprepisuje.** Ak verzia v registri už je, workflow značku neposunie a napíše varovanie. Je to zámer: Supervisor rozhoduje o aktualizácii podľa čísla verzie, takže vymeniť obsah pod tým istým číslom znamená, že prevádzkovateľ opravu nikdy nedostane — jednotka si bude myslieť, že je aktuálna. Vydanie preto znamená zvýšiť `version` v `genesis/config.yaml`.

### Čo ešte chýba k inštalácii

Publikovanie samo o sebe ešte nestačí. Repozitár je súkromný, takže **balík v GHCR je tiež súkromný** a Supervisor ho bez prihlásenia nestiahne. Sú dve cesty a treba sa rozhodnúť:

1. Zverejniť balík `os-genesis-pilot` v nastaveniach GitHub Packages. Obraz potom stiahne kdokoľvek; kód v súkromnom repozitári zostáva.
2. Nechať balík súkromný a pridať Supervisoru prihlasovacie údaje k `ghcr.io`. Supervisor to podporuje — drží si prihlasovacie údaje per registry ([`supervisor/api/docker.py`](https://github.com/home-assistant/supervisor/blob/main/supervisor/api/docker.py): `POST /docker/registry` s `{hostname: {username, password}}`, výpis cez `/docker/registries`). Token treba zaobchádzať ako s tajomstvom a nedávať ho do Gitu.

**Inštalácia na Home Assistant Green nie je overená.** Obraz je testovaný iba v GitHub CI emulácii; či ho Supervisor na skutočnom Green stiahne a spustí, nikto zatiaľ neskúšal — patrí to k ELYSIUM-350. Reálny Green, obnova po reštarte HA OS, skutočné zariadenie a merania CPU/RAM na hardvéri sú otvorené akceptačné kroky Jira ELYSIUM-338 a ELYSIUM-336.

## Kroky inštalácie

Predpokladom je, že balík je pre Supervisor dostupný podľa jednej z dvoch ciest vyššie.

1. V Home Assistant app obchode pridať URL repozitára `https://github.com/Elysium-a-s/OS-Genesis` (musí byť pre Supervisor dostupný).
2. Nainštalovať **OS Genesis Pilot** a v nastaveniach vložiť dva rôzne náhodné tokeny `read_token` a `write_token`, každý s aspoň 32 znakmi.
3. Spustiť app a otvoriť Health odkaz. V logoch skontrolovať `Home Assistant inventory loaded` po pripojení aspoň jedného svetla/zásuvky.
4. Reštartovať HA Green a overiť automatický štart, zachovanie databázy a aktuálny inventár.
5. Zaznamenať CPU/RAM a logy pred reštartom a po ňom; tajomstvá z logov nezdieľať.

HTTP port 8765 je určený iba pre dôveryhodnú domácu sieť. Nepresmerovávajte ho na verejný internet.
