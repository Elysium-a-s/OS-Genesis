# Genesis core

Minimálna Rust služba pre Linux. Poskytuje `GET /health` a tokenom chránené `GET /v1/devices`. Interný SQLite execution ledger eviduje prijatie povelu a pravdivé stavové prechody. Pilotné príkazy pre svetlo/zásuvku používajú samostatný write token, serverom určeného aktéra a execution ledger. Pilotné roly owner/member/guest sa vynucujú na API. Párovanie a revokácia tokenov patria do ELYSIUM-347. Health odpoveď nepotvrdzuje pripojenie k Home Assistantu ani stav zariadení.

## Lokálne spustenie

Vyžaduje stabilný Rust toolchain (rustup, cargo) a Linux alebo macOS na vývoj.

```sh
cd core
cargo run
curl -i http://127.0.0.1:8080/health
```

Očakávaná odpoveď: HTTP 200, `Content-Type: application/json`, telo s `status: ok`, `service: genesis-core` a `version`.

## Konfigurácia

| Premenná | Predvolené | Popis |
| --- | --- | --- |
| `GENESIS_BIND_ADDR` | `127.0.0.1:8080` | IP adresa a port posluchu; neplatná hodnota zastaví štart. |
| `GENESIS_LOG` | `genesis_core=info` | Filter štruktúrovaných JSON logov; neplatná hodnota zastaví štart. |
| `GENESIS_LEDGER_PATH` | `./genesis-ledger.sqlite3` | Trvalý SQLite súbor. Pre Home Assistant app nastaviť cestu v perzistentnom `/data`; zlyhanie otvorenia zastaví štart. |
| `GENESIS_HA_WS_URL` | nenastavené | HA WebSocket URL; nastavuje sa spolu s HA tokenom. |
| `GENESIS_HA_TOKEN` | nenastavené | HA access token; nesmie byť v Gite ani logoch. |
| `GENESIS_READ_TOKEN` | nenastavené | Samostatný token s aspoň 32 znakmi pre read-only inventár a snapshot povelu. Bez neho read endpointy vracajú 503. |
| `GENESIS_WRITE_TOKEN` | nenastavené | Pilotný owner token s aspoň 32 znakmi. Umožňuje čítanie a ovládanie. |
| `GENESIS_MEMBER_TOKEN` | nenastavené | Voliteľný member token s aspoň 32 znakmi. Umožňuje čítanie a ovládanie. |
| `GENESIS_GUEST_TOKEN` | nenastavené | Voliteľný guest token s aspoň 32 znakmi. Umožňuje iba čítanie. |
| `GENESIS_HOUSEHOLD_ID` | `pilot-home` | Jediná povolená pilotná domácnosť. |

Všetky nastavené prístupové tokeny musia byť navzájom odlišné. `GET /v1/me` vráti serverom určenú domácnosť, aktéra, rolu a `can_control_devices`. `POST /v1/commands` vracia guest/service role 403; `household_id` mimo pilotnej domácnosti je zamietnuté. Pilot používa jeden token na rolu, preto zatiaľ nerozlišuje konkrétnych členov v rovnakej role. Vydávanie tokenov a ich revokácia sú predmetom ELYSIUM-347. Pri expozícii mimo dôveryhodnej LAN je potrebné TLS a autentifikovaný prístupový kanál.

Predvolená adresa je dostupná iba lokálne. Pre Home Assistant app/kontajner môže byť potrebná adresa `0.0.0.0:8080`; chránený endpoint `/v1/devices` vyžaduje samostatný read token. Konfiguráciu držte v prostredí, nie v Gite. Core nikdy nevypisuje celé prostredie do logu.

Príklad vývojového spustenia na inom porte:

```sh
GENESIS_BIND_ADDR=127.0.0.1:8081 GENESIS_LOG=genesis_core=debug cargo run
```

## Home Assistant inventár

Ak sú nastavené obe HA premenné, core sa autentifikuje cez WebSocket, načíta svetlá a zásuvky a po odpojení sa opäť pripája. Podrobnosti a limity sú v [adaptéri](../adapters/home-assistant/README.md). Inventár čítajte s hlavičkou `Authorization: Bearer <GENESIS_READ_TOKEN>` na `GET /v1/devices`; token poskytujte bezpečným klientom, neukladajte ho do repozitára. Inventár vracia iba lokálny snapshot.

## Execution ledger

`core/src/ledger.rs` je interný modul pripravený pre budúci HA adaptér. `accept` atomicky uloží povel a prvú auditnú udalosť. Unikátny `(household_id, idempotency_key)` v SQLite zabezpečuje, že opakovanie rovnakého zámeru vráti pôvodný povel; iný zámer s rovnakým kľúčom je konflikt. Duplicita nevytvára ďalšiu udalosť ani nové ID povelu. `transition` atomicky uloží nový snapshot a auditnú udalosť s aktérom, UTC časom, korelačným ID a idempotency key. `sent` neznamená potvrdenie. `provider_confirmed` vyžaduje provider ack; `device_confirmed` vyžaduje pozorovanie zariadenia. Po `unknown` nie je povolený opätovný prechod do `sent`; neskorší dôkaz môže výsledok zosúladiť.

Ledger zatiaľ žiadny príkaz sám neodosiela a nemá HTTP endpoint. Idempotencia bráni opakovanému prijatiu, ale sama o sebe nedokazuje presne jedno fyzické vykonanie u externého poskytovateľa. Adaptér musí pri neistom výsledku použiť `unknown` a pred prípadným ďalším pokusom overiť stav zariadenia.

## Overenie

```sh
cd core
cargo fmt --check
cargo build
cargo test
```

GitHub Actions kontroluje formát, build, testy a kompiláciu pre cieľ `aarch64-unknown-linux-gnu`. Táto krížová kompilácia nie je dôkazom behu na Home Assistant Green; fyzické nasadenie patrí do samostatnej úlohy.

## Pilotný povel pre svetlo alebo zásuvku

`POST /v1/commands` vyžaduje `Authorization: Bearer <GENESIS_WRITE_TOKEN alebo GENESIS_MEMBER_TOKEN>` a JSON:

```json
{
  "household_id": "pilot-home",
  "device_id": "ha:light.living",
  "value": true,
  "idempotency_key": "pilot-on-001",
  "correlation_id": "manual-test-001"
}
```

Token nepridávajte do príkazového riadku, histórie shellu ani logov. Endpoint povoľuje iba existujúcu online HA entitu `light.*` alebo `switch.*` v pilotnej domácnosti. Identitu aktéra určuje core, nie JSON od klienta. Rovnaký idempotency key a zámer vrátia pôvodný povel bez druhého odoslania. Konfliktný zámer vráti 409. Snapshot možno čítať cez `GET /v1/commands/{command_id}` s read tokenom.

Stav `sent` je iba odoslanie WebSocket správy. `provider_confirmed` vyžaduje úspešnú odpoveď `call_service`. `device_confirmed` vyžaduje čerstvú udalosť `state_changed` so zhodným HA context ID, entitou a požadovanou hodnotou. Časový limit po odoslaní je `unknown`, aj keď provider už potvrdil prijatie. Pri výpadku pred odoslaním je výsledok `failed`. Fyzické zapnutie/vypnutie, Tuya/Smart Life správanie a koreláciu konkrétnej žiarovky treba overiť na Home Assistant Green.
