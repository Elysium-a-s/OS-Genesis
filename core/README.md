# Genesis core

Minimálna Rust služba pre Linux. V tejto fáze poskytuje iba `GET /health`. Interný SQLite execution ledger eviduje prijatie povelu a pravdivé stavové prechody. Inventár, autentifikované API, autorizácia a skutočné odosielanie príkazov do HA patria do ďalších Jira úloh. Health odpoveď nepotvrdzuje pripojenie k Home Assistantu ani stav zariadení.

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

Predvolená adresa je dostupná iba lokálne. Pre Home Assistant app/kontajner môže byť potrebná adresa `0.0.0.0:8080`; pred sprístupnením mimo zariadenia musí ďalšia etapa pridať autentifikáciu alebo sieťové obmedzenie. Konfiguráciu držte v prostredí, nie v Gite. Core zatiaľ nepotrebuje žiadne tokeny a nikdy nevypisuje celé prostredie do logu.

Príklad vývojového spustenia na inom porte:

```sh
GENESIS_BIND_ADDR=127.0.0.1:8081 GENESIS_LOG=genesis_core=debug cargo run
```

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
