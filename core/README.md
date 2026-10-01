# Genesis core

Minimálna Rust služba pre Linux. Poskytuje `GET /health` a tokenom chránené `GET /v1/devices`. Interný SQLite execution ledger eviduje prijatie povelu a pravdivé stavové prechody. Pilotné príkazy pre svetlo/zásuvku používajú samostatný write token, serverom určeného aktéra a execution ledger. Pilotné roly owner/member/guest sa vynucujú na API. Hlasový povel prechádza tou istou autorizáciou a tým istým ledgerom ako panel; Genesis pri tom neprijíma zvuk. Prístup sa vydáva párovaním s jednorazovým kódom a dá sa odobrať; tajomstvá sú v databáze iba ako odtlačok. Health odpoveď nepotvrdzuje pripojenie k Home Assistantu ani stav zariadení.

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
| `GENESIS_PANEL_DIR` | nenastavené | Voliteľný adresár so zostaveným Flutter web panelom. Musí obsahovať `index.html`; panel sa poskytuje na `/` z rovnakej adresy ako API. |
| `GENESIS_HOUSEHOLD_ID` | `pilot-home` | Jediná povolená pilotná domácnosť. |
| `GENESIS_BACKUP_DIR` | nenastavené | Priečinok pre zálohy databázy. Bez neho vracia `POST /v1/backup` 503. V Home Assistant app patrí pod perzistentné `/data`. |
| `GENESIS_SENSITIVE_DEVICES` | nenastavené | Zoznam `device_id` oddelený čiarkou, ktoré prevádzkovateľ označil za citlivé. Hlasová akcia na nich vyžaduje potvrdenie. Neplatná hodnota zastaví štart. |

Všetky nastavené prístupové tokeny musia byť navzájom odlišné. `GET /v1/me` vráti serverom určenú domácnosť, aktéra, rolu a `can_control_devices`. `POST /v1/commands` vracia guest/service role 403; `household_id` mimo pilotnej domácnosti je zamietnuté. Tokeny z konfigurácie sú **bootstrap tejto jednotky**: jeden na rolu, takže konkrétnych členov v rovnakej role nerozlišujú. Aktérov na osobu vydáva párovanie nižšie. Pri expozícii mimo dôveryhodnej LAN je potrebné TLS a autentifikovaný prístupový kanál.

Ak je `GENESIS_PANEL_DIR` nastavený, `GET /` a webové assety poskytujú Flutter panel. API zostáva na `/v1/*` a health na `/health`; chýbajúca API cesta nevracia HTML.

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

## Časovo obmedzený prístup

`core/src/grant.rs` vykoná rozhodnutie Behavior enginu ako dočasný unlock a pri expirácii ho vráti späť. Rozhodnutie musí prejsť kontraktom z `contracts/behavior/v1/decision.schema.json`; grant otvára iba operácia `apply` a iba vtedy, keď je okno `valid_from`–`expires_at` práve otvorené.

Grant prechádza stavmi `granted` → `active` → `relocked`. Neisté uzavretie skončí v `relock_pending`, nevykonaný unlock v `unlock_failed` a grant, ktorého zariadenie drží novšie rozhodnutie, v `superseded`. Unlock aj relock idú cez ten istý execution ledger, takže platia rovnaké pravidlá dôkazov: `provider_confirmed` vyžaduje provider ack, `device_confirmed` pozorovanie zariadenia. Za potvrdený sa výsledok považuje až vtedy, keď dosiahne úroveň, ktorú rozhodnutie žiada v `required_confirmation`.

Idempotencia je odvodená od rozhodnutia. Unlock používa `idempotency_key` z rozhodnutia, uzavretie ten istý kľúč s príponou `:relock` a opakovaný pokus ešte s číslom pokusu (`:relock2`); kľúč preto nesmie mať viac než 120 znakov. Opakované doručenie toho istého rozhodnutia vráti pôvodný grant a nevytvorí druhý povel, opakované zapísanie výsledku nič nemení.

Neistý unlock (`unknown`, alebo len provider ack tam, kde rozhodnutie žiada zariadenie) sa zámerne považuje za otvorený prístup a grant sa pri expirácii aj tak zamyká — relock navyše je bezpečnejší než odomknuté zariadenie. Naopak `failed` unlock sa nevykonal, takže stav prejde do `unlock_failed` a nič sa nevracia.

Ak relock nedosiahne vyžadované potvrdenie, grant skončí v `relock_pending` a vznikne incident `relock_uncertain`. Incident má odvodené id, takže opakovaný rovnaký výsledok nezaloží druhý záznam. Odtiaľ pokračuje zosúladenie.

Relock nespúšťa HTTP požiadavka. Ak sú nastavené HA premenné, core spustí plánovač, ktorý každých 30 sekúnd zamkne všetko po expirácii a skúsi zosúladiť neisté relocky; tik teda určuje, o koľko neskôr než `expires_at` sa zariadenie zamkne. Prechod drží zámok ledgeru rovnako ako obsluha povelu, čo pri pilotnej jednej domácnosti stačí.

## Zosúladenie fyzického stavu

`core/src/grant.rs` a `ha_command::reconcile` dorovnávajú fyzický stav po reštarte, strate Home Assistanta, zmazaní cieľa, override a expirácii. Celý modul berie čas ako parameter, nie zo systémových hodín, takže sa každý scenár dá odohrať v teste.

**Reštart.** Pri štarte core zavolá `grant::resume`. Po štarte nie je nič na ceste, takže povel, ktorý zostal v `accepted` alebo `sent`, sa prizná ako `unknown` s dôvodom `interrupted_before_result` a vznikne incident `interrupted_by_restart`. Grant s prerušeným unlockom prechádza do `active`, teda medzi tie, ktoré treba pri expirácii zamknúť. Beží to aj bez HA premenných, pretože samo nič neposiela.

**Strata Home Assistanta.** Prvé uzavretie sa zapíše vždy, aj keď zariadenie nie je dostupné — inak by o možnom otvorenom prístupe nič nesvedčilo. Opakovaný pokus na nedostupné zariadenie sa už neposiela a nepočíta sa medzi pokusy; incident zostáva otvorený, kým sa zariadenie neozve.

**Pozorovanie namiesto povelu.** Keď inventár vidí zariadenie online v hodnote, ktorou sa prístup zatvára, grant sa uzavrie dôkazom a nepošle sa nič. To je aj jediná cesta, ako sa uzavrie relock zariadenia, ktoré už v čase expirácie bolo v správnej hodnote: HA v takom prípade nevydá `state_changed`, takže povel sám by skončil ako `unknown`.

**Opakované pokusy.** Neistý relock sa skúša najviac päťkrát, prvý opakovaný pokus po minúte a každý ďalší po dvojnásobku predchádzajúceho. Každý pokus má vlastný povel aj idempotency kľúč. Po vyčerpaní limitu Genesis už nič neposiela a incident `relock_exhausted` zostáva otvorený pre človeka.

**Zmazanie cieľa a override.** Rozhodnutie s `operation=revert` uzavrie prístup ešte pred expiráciou. Jeden povel uzatvára všetky granty, ktoré na danom zariadení a schopnosti môžu byť otvorené; revert bez otvoreného grantu nespraví nič. Override novším rozhodnutím na to isté zariadenie a hodnotu s dlhším oknom nechá starší grant prejsť do `superseded` — starší grant nesmie zamknúť zariadenie, ktoré novšie rozhodnutie legitímne drží, a novší grant má vlastnú expiráciu. Rozhodnutie, ktoré by na tom istom zariadení držalo protikladnú hodnotu, sa odmietne.

Prehľad je na `GET /v1/access` s ktorýmkoľvek platným tokenom. Vracia stav grantu, `required_confirmation`, počet uzatváracích pokusov, posledný potvrdený stav (povel, hodnota, úroveň potvrdenia a čas dôkazu) a otvorené incidenty; odpoveď je ohraničená na 200 grantov domácnosti.

### Vyžiadané zosúladenie jedného grantu

`POST /v1/access/{decision_id}/reconcile` spustí jeden prechod nad jedným grantom. Je to pre človeka, ktorý vidí otvorený incident a nechce čakať na ďalší periodický prechod. Smie to **iba vlastník** — nie preto, že by to bolo nebezpečné, ale preto, že je to zásah do fyzického sveta domácnosti a audit má povedať kto. Člen, hosť aj služba dostanú 403, a rola sa kontroluje skôr než čokoľvek iné: bez nastaveného Home Assistanta dostane vlastník 503, ale člen stále len 403, takže z odpovede nezistí ani to, či je prepojenie nastavené.

Volanie ide tou istou cestou ako periodický prechod, len s jedným grantom — zámerne, aby nedokázalo nič, čo plánované zosúladenie nerobí:

* **Posiela sa výlučne uzatváracia hodnota.** Prístup sa týmto nedá otvoriť ani predĺžiť.
* **Platné okno sa neskracuje.** Grant, ktorý by periodický prechod ešte nevybral, vráti `not_due` a nič sa nepohne. Zatvoriť prístup pred expiráciou nie je zosúladenie — je to odobranie prístupu a to má vlastné rozhodnutie `revert`, vlastný audit a vlastnú autorizáciu.
* **Limit pokusov ani backoff sa neobchádza.** Opakované volanie skončí na tej istej podmienke ako prechod a vráti `unchanged`; zariadenie sa tým zaplaviť nedá.

Odpoveď je `{"outcome": …, "access": …}`, kde `access` má ten istý tvar ako jeden prvok `GET /v1/access`. `outcome` je jedno z:

| `outcome` | čo to znamená |
| --- | --- |
| `settled` | Grant dosiahol koncový stav: prístup je dokázateľne zatvorený. |
| `attempted` | Uzavretie je zapísané, ale vyžadované potvrdenie nedošlo. Grant zostáva `relock_pending` a incident otvorený. |
| `unchanged` | Nič sa nepohlo: zariadenie je nedostupné, čaká sa na ďalší pokus alebo je limit vyčerpaný. |
| `not_due` | Okno grantu ešte platí. |
| `not_open` | Grant už nie je otvorený, takže nie je čo zosúlaďovať. |

`settled` a `attempted` sa zámerne nezlievajú do jedného „uspelo": prvé hovorí o fyzickom svete, druhé iba o tom, že sa Genesis pokúsil. Neznámy `decision_id` je 404.

### Bezpečnostné výnimky

1. Zosúladenie posiela iba opak hodnoty, ktorú grant otvoril. Nikdy neodomyká — na odomknutie treba nové rozhodnutie `apply`.
2. Neistý výsledok sa vždy počíta ako možný otvorený prístup. Chýbajúci dôkaz nie je dôkaz o zatvorení.
3. Provider ack nenahradí potvrdenie zariadením tam, kde to rozhodnutie žiada, a to ani pri zosúladení.
4. Opakovaný pokus sa neposiela na zariadenie, ktoré nie je online a zapisovateľné.
5. Pokusy sú ohraničené. Genesis zariadenie nebombarduje; nezosúladený stav je vec človeka, nie ďalšieho pokusu.
6. Pozorovanie z inventára sa prijíma ako dôkaz zariadenia, hoci nie je korelované s konkrétnym povelom: dokazuje fyzický stav, nie to, ktorý povel ho spôsobil. V audite to drží referencia `inventory:<čas>`. Povel, ktorý sa pri tom neodoslal, prechádza cez `unknown` s dôvodom `not_sent_device_already_in_the_closing_value`, pretože `sent` by bola lož.
7. Revert smie prístup iba zatvoriť. Rozhodnutie, ktoré by obnovilo hodnotu otvorenú grantom, sa odmietne.
8. Reštart nikdy nepovažuje prerušený povel za úspešný. Vyžiadané zosúladenie grantu bez zapísaného výsledku najprv zavolá `grant::resume`, takže sa nezatvára stav, o ktorom ešte nevieme, ako skončil.
9. Relock pri expirácii a zosúladenie vydáva aktér `genesis-core`, nie `behavior-engine`; withdraw nesie vydavateľa rozhodnutia. Audit má ukázať, kto povel skutočne vydal.
10. Vyžiadané zosúladenie smie iba vlastník, nekrátí platné okno a neobchádza limit pokusov. Je to tá istá cesta ako periodický prechod, len nad jedným grantom.

### Limity

Rozhodnutia zatiaľ nemajú HTTP endpoint ani inú prepravu — `apply_decision` a `withdraw_decision` volá zatiaľ len test. Ak sú pokusy vyčerpané a posledný uzatvárací povel skončil ako `failed`, pozorovanie grant neuzavrie, pretože ledger z `failed` nikam neprechádza; čaká človek. Revert doručený mimo vlastného okna je zamietnutý, grant sa potom zamkne až pri svojej expirácii. Fyzický cyklus initial lock → dôkaz → unlock → expirácia → relock → zosúladenie treba overiť na Home Assistant Green; krížová kompilácia ani testy to nenahrádzajú.

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

## Inventár domácnosti

`GET /v1/inventory` vracia domácnosť, jej miestnosti a jej zariadenia v jednej odpovedi, s ktorýmkoľvek platným tokenom:

```json
{
  "household": {"household_id": "pilot-home", "name": "Doma"},
  "areas": [{"area_id": "ha:living_room", "name": "Obývačka", "device_ids": ["ha:light.living"]}],
  "devices": [{"device_id": "ha:light.living", "area_id": "ha:living_room", "area_name": "Obývačka"}],
  "home_assistant": {"state": "connected", "rooms_incomplete": false}
}
```

V jednej odpovedi preto, že miestnosti a zariadenia musia byť z toho istého okamihu; dvoma dotazmi sa dá dostať zoznam miestností a k nemu zariadenie ukazujúce do miestnosti, čo medzitým zanikla. `home_assistant.state` je tam preto, aby sa **prázdny inventár nedal prečítať ako prázdna domácnosť**: „nič nevidíme" a „nič tam nie je" vyzerajú v odpovedi inak identicky. `rooms_incomplete` znamená, že registre sa nepodarilo prečítať celé — chýbajúce miestnosti sú priznané, nie vydávané za domácnosť bez miestností.

Miestnosť nemá vlastný názov v Genesise; berie sa taká, aká je nastavená v Home Assistante. Mapovanie je popísané v [adaptéri](../adapters/home-assistant/README.md). Zariadenie bez miestnosti má `area_id: null` a Genesis mu žiadnu nevyrába. Prázdna miestnosť v odpovedi zostáva — v domácnosti existuje aj vtedy, keď v nej zatiaľ nič nie je.

Čítanie stačí s ktorýmkoľvek platným tokenom, rovnako ako `GET /v1/access`: rola rozhoduje o ovládaní, nie o tom, či člen domácnosti vidí, čo v nej je. Domácnosť je jedna na jednotku a token je na ňu naviazaný, takže iná domácnosť sa do odpovede dostať nemôže. `GET /v1/devices` vracia ten istý tvar zariadenia vrátane `area_id` a `area_name`, len bez miestností a domácnosti.

## Prehľad povelov

`GET /v1/commands` vracia posledné povely domácnosti, najnovší prvý, s ktorýmkoľvek platným tokenom — je to čítanie. Radí sa podľa posledného zápisu do ledgeru, nie podľa času prijatia, takže vedie to, čo sa naposledy hýbalo. `limit` je voliteľný, predvolene 20 a najviac 50; vyššia hodnota sa zreže, nezamietne. Iné domácnosti sa do odpovede nedostanú.

Bez tohto sa dá prečítať iba povel, ktorého `command_id` už niekto má. Po obnovení panelu ani po reštarte jednotky ho nemá nikto, takže neistý povel by zostal ležať bez toho, aby sa o ňom niekto dozvedel.

## Rozhodnutie Elysium Behavior

`POST /v1/behavior/decisions` prijme verziovaný kontrakt z `contracts/behavior/v1/decision.schema.json` a vykoná ho ako časovo obmedzený grant. Nie je to endpoint pre domácnosť: volá ho Behavior engine, a preto má vlastný kanál, nie rolové tokeny.

Kanál nesie tri veci, ktoré holý bearer token nenesie: **identitu** (ktorý kľúč podpísal), **integritu** (že telo po podpise nikto nezmenil) a **odolnosť voči zopakovaniu** (podpis staršieho tela prestane fungovať). Token je tajomstvo, ktoré stačí raz zahliadnuť v logu proxy a ovláda domácnosť; podpis sám nie je oprávnenie na nič iné než na to jedno telo v tom jednom okne.

```
POST /v1/behavior/decisions
X-Genesis-Key-Id: behavior-1
X-Genesis-Timestamp: 2026-09-30T18:00:00Z
X-Genesis-Signature: <hex HMAC-SHA256>
```

Podpisuje sa kanonický text `v1:POST:/v1/behavior/decisions:<časová značka>:<telo>`. Verzia preto, aby sa tvar dal zmeniť bez toho, aby starý podpis zostal platný; cesta preto, aby sa podpis nedal preniesť na iný endpoint; časová značka preto, aby sa nedala vymeniť za novšiu. Značka smie byť najviac päť minút od času jednotky. Telo sa **neparsuje pred overením**: podpis pokrýva presne tie bajty, ktoré prišli, takže overiť znovu poskladaný JSON by znamenalo kontrolovať niečo iné.

| premenná | |
| --- | --- |
| `GENESIS_BEHAVIOR_KEY_ID` | ktorý kľúč jednotka pozná; nie je to tajomstvo |
| `GENESIS_BEHAVIOR_SECRET` | spoločné tajomstvo, aspoň 32 znakov |
| `GENESIS_CENTRAL_UNIT_ID` | voliteľné; keď je nastavené, rozhodnutie adresované inej jednotke tej istej domácnosti sa odmietne |

Obe Behavior premenné sa nastavujú spolu. Bez nich endpoint vracia 503 — jednotka bez prepojenia na Behavior nie je pokazená.

Odpoveď nesie **aj odmietnutie, aj neistotu**, pretože Behavior si z „prešlo to" nesmie vyvodiť, že sa vo svete niečo stalo:

| `outcome` | čo to znamená |
| --- | --- |
| `granted` | zariadenie zmenu potvrdilo |
| `granted_unconfirmed` | prístup je otvorený, potvrdenie zariadením nedorazilo |
| `not_executed` | povel sa nevykonal, prístup zostáva zavretý |
| `withdrawn`, `nothing_to_withdraw` | výsledok `revert` rozhodnutia |
| `refused` | rozhodnutie sa nevykonalo; `reason_code` a `detail` hovoria prečo |

Snapshot povelu je v odpovedi celý, vrátane `unknown` a `failed`. Vykonané rozhodnutie vracia 200 aj vtedy, keď je výsledok neistý — požiadavka je spracovaná a v ledgeri; neistota je v tele, nie v stave. Odmietnutie je 4xx: 401 podpis, 403 cudzia domácnosť alebo iná jednotka, 409 zatvorené okno alebo nepodporovaná schopnosť, 400 pokazený kontrakt.

Log z tohto endpointu obsahuje iba kategóriu odmietnutia. Ani podpis, ani telo, ani očakávanú hodnotu — z logu sa nemá dať zložiť platná požiadavka.

## Diagnostika

`GET /v1/diagnostics` s ktorýmkoľvek platným tokenom vracia stav jednotky a **oddelene** stav prepojenia na Home Assistant:

```json
{
  "unit": {"version": "0.1.0", "household_id": "pilot-home", "home_assistant_configured": true},
  "home_assistant": {
    "state": "disconnected",
    "since": "2026-09-30T09:00:00+00:00",
    "last_inventory_at": "2026-09-30T08:55:00+00:00",
    "last_inventory_devices": 3,
    "last_error": "authentication"
  }
}
```

`state` je `not_configured`, `connecting`, `connected` alebo `disconnected`. `connected` znamená načítaný celý inventár, nie otvorený socket. `since` sa pri opakovaných pokusoch nehýbe, takže sa z neho dá prečítať dĺžka výpadku. `last_error` je iba kategória (`connection`, `authentication`, `protocol`, `disconnected`) — adresa ani token sa do diagnostiky nedostanú.

Je to zámerne iný endpoint než `/health`. `/health` odpovedá na jednu otázku — či beží HTTP server — a musí zostať bez tokenu, pretože ho volá watchdog Supervisora. Stav prepojenia na Home Assistant je údaj o domácnosti, takže si žiada prístup; keby ho niesol `/health`, čítal by ho každý, kto sa dostane na port. A hlavne: zdravá jednotka nie je dôkaz dostupného Home Assistanta, takže jedna zelená kontrolka pre oboje by svietila nad inventárom, ktorý sa už nehýbe.

## Hlasový povel

`POST /v1/voice/commands` vezme prepis reči a vykoná ho tou istou cestou ako panel. Rozpoznávanie reči patrí hlasovému rozhraniu — v pilote Home Assistant Assist — a **Genesis neprijíma zvuk**: telo požiadavky má iba text, neznáme polia sa zamietajú, takže požiadavku so zvukom nie je možné ani poslať. STT/TTS adaptér je samostatná úloha epicu.

```json
{
  "household_id": "pilot-home",
  "transcript": "Zhasni svetlo v obývačke",
  "idempotency_key": "voice-001",
  "correlation_id": "assist-001",
  "store_transcript": false
}
```

Autorizácia je rovnaká ako pri paneli: vyžaduje `GENESIS_WRITE_TOKEN` alebo `GENESIS_MEMBER_TOKEN`, guest a service dostanú 403, `household_id` mimo pilotnej domácnosti je zamietnuté a identitu aktéra určuje core. Vykonanie ide cez `ha_command::run_power_command`, ktorý používa panel aj hlas, takže kontrola vykonateľnosti, idempotencia, ledger aj dôkazy sú pre oboch rovnaké. Prepis nedostáva žiadne právo, ktoré by nemal panel.

### Z prepisu na typovaný intent

Prevod je deterministický: uzavretý zoznam slov, žiadny model. Text sa prevedie na malé písmená, odstráni sa diakritika (rozpoznávanie reči ju vracia nespoľahlivo) a z každého slova sa zahodí jedna koncová samohláska, takže `obývačke` aj `obývačka` vedú na ten istý základ. Zariadenie sa hľadá v tomto poradí:

1. **Podľa názvu z Home Assistanta.** Všetky významné slová názvu musia byť v povele. Jedna zhoda sa vykoná, viac zhôd nie.
2. **Podľa druhu zariadenia** (`svetlo`, `lampa`, `zásuvka`, `light`, `plug`…), a to len vtedy, keď povel neobsahuje nič iné než druh. „Zhasni svetlo v spálni" preto nezhasne svetlo v obývačke ani vtedy, keď je to jediné svetlo: spálňu Genesis nepozná, takže správna odpoveď je nevykonať nič.
3. Inak sa nevykoná nič.

Vykonaný povel vracia 200 s intentom a snapshotom povelu. Nejednoznačný povel vracia **422**, nie 200 — klient ho nesmie pochopiť ako vykonaný — s dôvodom `unrecognised_action`, `conflicting_action`, `no_matching_device` alebo `several_matching_devices`; pri poslednom aj so zariadeniami, medzi ktorými sa Genesis nerozhodol, aby sa dalo doplniť otázku. Samotné sloveso bez cieľa neprepne ani jediné zariadenie.

Každý výsledok nesie `outcome` a vetu pre používateľa: `executed`, `confirmation_required`, `unclear` alebo `refused`. Pri odmietnutí je kód v `reason` (nejednoznačnosť) alebo v `explanation.code` (ostatné); je to ten istý druh údaja na dvoch miestach, pretože nejednoznačnosť nesie aj zariadenia, medzi ktorými sa nerozhodlo.

### Citlivé akcie a potvrdenie

Citlivá akcia sa nevykoná na prvé slovo. Genesis ju odloží, vráti **202** s `confirmation_id`, typovaným intentom a časom platnosti, a čaká, kým ju ten istý oprávnený člen výslovne potvrdí na `POST /v1/voice/confirmations`:

```json
{ "household_id": "pilot-home", "confirmation_id": "…" }
```

Čo je citlivé, rozhoduje prevádzkovateľ cez `GENESIS_SENSITIVE_DEVICES`: Genesis nevie, čo je za zásuvkou, vie to ten, kto ju zapojil. Nad tým platí zoznam vlastne citlivých domén Home Assistanta (`lock.`, `cover.`, `valve.`, `water_heater.`, `climate.`, `alarm_control_panel.`); pilot mapuje iba `light.` a `switch.`, takže dnes môže citlivé zariadenie vzniknúť **iba deklaráciou**. Citlivé je oboje — zapnutie aj vypnutie; univerzálny bezpečný smer neexistuje, bojler je nebezpečné zapnúť aj vypnúť.

Potvrdenie platí dve minúty, presne raz a iba pre aktéra, ktorý o akciu požiadal. Označí sa za použité **pred** vykonaním, takže po neistom výsledku treba povel povedať znova; opakovateľné potvrdenie by bolo horšie než druhé vyslovenie. Prepis, ktorý si odložená akcia so súhlasom držala, sa po použití alebo expirácii zahodí — potvrdenie, ktoré už nemôže nič vykonať, nemá dôvod držať slová.

### Vysvetlenie výsledku

Odpoveď rozlišuje odoslanie od potvrdenia. `explanation.code` kopíruje stav povelu v ledgeri (`sent`, `provider_confirmed`, `device_confirmed`, `unknown`, `failed`) a `explanation.message` je veta, ktorú môže hlasové rozhranie povedať. Žiadna z nich netvrdí viac, než ledger vie: `sent` neznie ako hotovo a `unknown` neznie ako zlyhanie.

### Audit

`GET /v1/voice/audit` vracia posledných 200 rozhodnutí domácnosti: kto, čo sa rozhodlo (`executed`, `awaiting_confirmation`, `refused`), prečo, a keď je to známe, zariadenie a povel. Stačí ktorýkoľvek platný token, pretože je to čítanie.

Záznam **neobsahuje prepis ani identifikátor potvrdenia**, a nemá na ne ani stĺpec. Dôvod je vždy kód zo zatvoreného zoznamu, nikdy text od používateľa. Audit býva presnejší než odpoveď: potvrdenie, ktoré patrí inému aktérovi, sa zapíše ako `foreign_confirmation`, ale volajúcemu sa povie iba `unknown_confirmation` — že taký identifikátor existuje, sa dozvedieť nemá.

Zapisujú sa aj zamietnutia, ktoré padnú ešte pred spracovaním povelu (`role_not_permitted`, `other_household`). Požiadavka **bez platného tokenu** záznam nevytvorí: Genesis nevie, komu by ho pripísal, a audit plnený anonymnými volajúcimi by sa dal beztrestne nafúknuť.

### Súhlas a prepis

Prepis sa **neukladá ani nezapisuje do logov**. Uloží sa iba vtedy, keď požiadavka nesie `store_transcript: true`, teda keď používateľ súhlas dal, a aj potom len k povelu, ktorý sa naozaj vykonal. Nejednoznačný povel neuloží nič — nevznikne povel, záznam o hlase ani prepis.

Že povel prišiel hlasom, sa zapisuje vždy, spolu s typovaným intentom (zariadenie a hodnota, nie slová). Audit tak vie o hlase aj vtedy, keď o vyslovenom vedieť nesmie. Idempotency kľúč, ktorý už patrí existujúcemu povelu, sa označí za hlasový len vtedy, keď ten povel vznikol týmto hlasovým povelom; inak by audit tvrdil o panelovom povele, že ho niekto vyslovil.

### Limity

Zoznam slov je uzavretý a pokrýva slovenské rozkazovacie formy zapnutia a vypnutia plus anglické `turn/switch on|off`; synonymá, iné jazyky a iné akcie než `power` nie sú podporované. Pilot má jeden token na rolu, takže „ten istý člen" znamená tá istá rola — dvoch členov zdieľajúcich token Genesis nerozlíši; token z konfigurácie osobu nenesie. S vydanou kreditívou (párovanie nižšie) už „ten istý člen" znamená konkrétneho aktéra. Či má citlivú akciu potvrdzovať prísnejšia rola než tá, ktorá o ňu požiadala, je produktové rozhodnutie a zostáva otvorené. Audit nemá retenciu, iba ohraničenú odpoveď. Miestnosti a skupiny Genesis nepozná, pozná iba názvy zariadení z Home Assistanta, takže povel bez názvu alebo druhu zariadenia sa nevykoná. Odmietnutý povel neukladá nič, takže z neho nie je z čoho zlepšovať rozpoznávanie. Zapojenie na skutočný Home Assistant Assist ani hlasový povel na fyzickom zariadení zatiaľ neboli overené; patrí to k pilotu na Home Assistant Green.

## Párovanie a prístupové tokeny

Prístup ku Genesis sa nezískava tým, že si niekto prečíta konfiguráciu. Vlastník domácnosti spustí párovanie, dostane **jednorazový kód**, a ten sa raz vymení za prístupový token pre konkrétneho aktéra. Token sa dá kedykoľvek odobrať. Návrh a jeho dôvody sú v [docs/ELYSIUM-347-identita.md](../docs/ELYSIUM-347-identita.md).

1. Vlastník spustí párovanie: `POST /v1/pairings` s `{"household_id","role","actor_id"}`. V odpovedi je `code` — práve raz.
2. Klient kód vymení: `POST /v1/pairings/redeem` s `{"household_id","code"}`. V odpovedi je `token` — práve raz. Tento endpoint nepotrebuje token, pretože kód sám je oprávnenie.
3. Vlastník vidí vydané kreditívy na `GET /v1/credentials` a ktorúkoľvek odoberie cez `DELETE /v1/credentials/{credential_id}`.

Kód ani token nedávajte do príkazového riadku, histórie shellu ani logov. Pri expozícii mimo dôveryhodnej LAN platí to isté ako pre ostatné endpointy: bez TLS ich vidí sieť.

Párovanie aj odobranie je **iba pre vlastníka**; member, guest a service dostanú 403. Vlastník smie vydať aj ďalšieho vlastníka — druhý vlastník domácnosti je legitímny stav. Kód platí desať minút a vymení sa presne raz; nepoužiteľný kód vracia 422 bez toho, aby prezradil, čo mu chýba. Odobraná kreditíva je pri ďalšej požiadavke 401.

Vydaná kreditíva nesie `actor_id`, takže `GET /v1/me`, execution ledger aj hlasový audit hovoria **kto**, nie iba akou rolou. Prehľad kreditív ukazuje `issued_at`, `last_used_at`, `revoked_at` a `revoked_by`, aby sa dalo rozhodnúť, čo je ešte potrebné.

### Tajomstvá

V databáze je z kódu aj z tokenu iba SHA-256 odtlačok. Ani úplný výpis tabuliek nedá hodnotu, ktorou sa dá vojsť, a Genesis token nedokáže zopakovať — kto ho stratí, spraví nové párovanie. Do logu sa píše `pairing_id`, `credential_id`, rola a aktér; kód ani token nikdy. V Gite nie je ani jedno, pretože obe vznikajú až za behu.

Prečo stačí jeden SHA-256 a nie zdržiavacia funkcia: tokeny nie sú heslá, sú to náhodné hodnoty s viac než dvomi stovkami bitov entropie. Proti nim nemá slovníkový ani hrubý útok o čo sa oprieť.

### Cudzia domácnosť

Kreditíva nesie domácnosť, pre ktorú bola vydaná. Jednotka prijme iba kreditívu svojej domácnosti, takže prenesená databáza cudziu domácnosť neovládne. Povel s `household_id` inej domácnosti je 403 aj vtedy, keď rola na ovládanie stačí, a kód jednej domácnosti nevydá kreditívu pre druhú.

### Limity

Tokeny z konfigurácie zostávajú a sú prvé v poradí — sú bootstrapom, ktorým vlastník vôbec spustí prvé párovanie, a pilotný Home Assistant app ich nastavuje v možnostiach. Odobrať sa nedajú inak než zmenou konfigurácie a reštartom; kým sú nastavené, sú to plnohodnotné prístupy bez záznamu o vydaní. Vydané kreditívy nemajú expiráciu, iba odobranie. Výmena kódu nemá rate limiting — kód má vyše sto bitov entropie, takže hádanie nie je cesta, ale keby sa mal kód niekedy zadávať rukou, a teda skrátiť, rate limiting sa stane podmienkou. Rotácia tokenu je dnes „vydaj nový, odober starý", nie samostatný tok.

## Záloha, aktualizácia a obnova

Celý stav Genesis je jeden SQLite súbor: povely a ich audit, granty, hlasové záznamy a vydané kreditívy. Postup aj to, čo Genesis po reštarte urobí sám, je v [docs/ELYSIUM-348-obnova.md](../docs/ELYSIUM-348-obnova.md).

`POST /v1/backup` s owner tokenom vypíše konzistentnú kópiu do `GENESIS_BACKUP_DIR` a vráti cestu a veľkosť. Kópia vzniká cez SQLite `VACUUM INTO`, takže je celá a platná aj vtedy, keď sa práve zapisuje — **`cp` za behu nie je záloha**. Existujúci súbor sa neprepíše (409). Záloha je plnohodnotná databáza, takže sa dá otvoriť a overiť bez obnovy.

`GET /v1/backup` vracia, čo jednotka má, najnovšie prvé, ohraničené na 50 záznamov. Tiež iba owner: cesty sú o súborovom systéme jednotky a komu patrí záloha, patrí aj jej zoznam. Tajomstvo v odpovedi nie je — meno súboru je iba čas vzniku.

Čas v prehľade sa berie **z mena súboru**, nie z času úpravy: ten sa dá zmeniť kopírovaním aj `touch`-om. Meno sa skladá a rozoberá na jednom mieste (`backup::file_name` a `backup::moment_from`), takže sa tie dve pravidlá nemôžu rozísť, a dvojbodky sa vracajú iba do časovej časti za `T` — dátum spojovníky nesie legitímne. Súbor, ktorý sa nedá prečítať ako záloha Genesis, sa z prehľadu zahodí namiesto toho, aby sa hlásil ako záloha.

Nenastavený `GENESIS_BACKUP_DIR` je 503, nie prázdny zoznam. Prázdny zoznam znamená, že záloha ešte nebola; to prvé je stav jednotky, to druhé stav domácnosti, a zliať ich by znamenalo tvrdiť, že zálohovanie funguje a nikto ho nepoužil.

Obnovu robí prevádzkovateľ pri zastavenej službe; Genesis ju úmyselne nevie spustiť sám, pretože podsunúť si súbor pod otvoreným spojením je cesta k poškodeniu.

Databáza si pamätá verziu schémy v `PRAGMA user_version`. Aktualizácia si schému doplní pri otvorení a nič nemaže. **Staršia verzia novšiu databázu odmietne otvoriť** namiesto toho, aby pracovala s obsahom, o ktorom nevie — rollback preto znamená obnoviť zálohu spravenú pred aktualizáciou.

Po štarte sa otvorené povely zosúladia: granty si svoje prevezme `grant::resume` (incident a rozhodnutie o stave), ostatné — z panela a z hlasu — doberie `Ledger::adopt_interrupted` a prizná ich ako `unknown` s dôvodom `interrupted_before_result`. Žiadny sa nevydáva za vykonaný.

**Čas návratu nie je odmeraný.** Je to pilotné meranie na Home Assistant Green; čo presne merať, je v odkazovanom dokumente. Rovnako nie je overená obnova po skutočnom výpadku napájania a záloha nemá plán ani rotáciu — endpoint ju vytvorí, ale sám sa nevolá.
