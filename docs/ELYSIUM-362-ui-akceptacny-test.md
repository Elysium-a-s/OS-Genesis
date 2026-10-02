# ELYSIUM-362 — UI akceptačný test aktuálne vyvinutých funkcií

Dátum behu: **2026-10-02**. Všetkých deväť prípadov **PASS**.

Test prešiel panelom v skutočnom prehliadači proti skutočnej jednotke. Nie je to
simulácia: panel je `flutter build web --release` v Chromiu, jednotka je
preložená binárka a hovoria spolu po sieti. Spúšťa sa cez
`sh tests/ui-acceptance/run.sh`; ako to funguje a prečo, je v
`tests/ui-acceptance/README.md`.

## Verzie

| | |
|---|---|
| Vetva / základ | `claude/elysium-362-ui-acceptance` z `0ac3dad` |
| Jednotka | `genesis-core 0.1.0`, rustc 1.94.1 |
| Panel | Flutter 3.47.5 stable, release web build |
| Prehliadač | Chromium 141.0.7390.37 |
| HA app | `0.1.5` (obsah vydania; ELYSIUM-361 a 362 v ňom nie sú) |
| Testy popri tom | 57 panelových, 119 na jednotke — všetky zelené |

## Výsledky

| # | Prípad | Rozsudok |
|---|---|---|
| 1 | Panel na `/`, API na `/v1`, health na `/health` | PASS |
| 2 | Owner/member/guest, odmietnutie guest zápisu a cudzej domácnosti | PASS |
| 3 | Párovanie kódom, uplatnenie, expirácia a odobranie | PASS |
| 4 | Inventár, miestnosti, výpadok Home Assistanta a obnova | PASS |
| 5 | Zapnutie/vypnutie z panela, ledger, neistý výsledok | PASS |
| 6 | Behavior grant, expiry a relock | PASS |
| 7 | Hlasový povel, nejednoznačnosť, citlivá akcia, audit | PASS |
| 8 | Prevádzkové centrum, záloha, zákaz pre člena a hosťa | PASS |
| 9 | Reštart aplikácie aj jednotky bez straty incidentov | PASS |

Podrobnosti ku každému prípadu sú nižšie. Hlavné kroky sa robili cez panel; API
volania slúžia len ako kontrolný dôkaz serverovej autorizácie a stavu, ako tiket
pripúšťa.

## Dve chyby, ktoré test našiel

### 1. Panel sa v domácnosti bez internetu nenakreslil vôbec

`flutter build web --release` si CanvasKit (7,2 MB `.wasm`) načítaval
z `https://www.gstatic.com/flutter-canvaskit/…`, hoci ten istý súbor je v obraze
priložený. Bez internetu stránka nespadla do náhradného vykreslenia — skončila
prázdna, s `Failed to fetch dynamically imported module`.

Pre produkt, ktorý má ovládať domácnosť z lokálnej siete, je to vážne: jednotka
aj žiarovka sú na mieste a fungujú, ale panel nenakreslí nič.

CI to nemohlo zachytiť — smoke test cez `curl` overoval `<base href>` v HTML
a JavaScript nikdy nespustil. Panel nebol do dnes v prehliadači ani raz.

Oprava: `--no-web-resources-cdn` v oboch workflowoch a kontrola, ktorá zlyhá, ak
sa odkaz na CDN vráti. Overené v prehliadači s odrezaným internetom: panel sa
nakreslí celý a **nepokúsi sa o žiadne spojenie mimo jednotky**.

### 2. Párovací kód sa nedal prečítať čítačkou obrazovky

`SelectableText` na rozdiel od `Text` nedáva svoj obsah do stromu prístupnosti.
Kód sa pritom zobrazuje **práve raz** a jednotka ho druhýkrát nevydá, takže
nevidiaci vlastník nemal ako pridať člena do domácnosti.

Oprava: `semanticsLabel` s kódom po štvoriciach, aby ho čítačka nečítala ako
jeden zhluk. Vizuálne zobrazenie zostáva nezmenené. Widget test v
`panel/test/access_test.dart` stráži, že popis existuje a nesie ten istý kód.

## Čo tento test nedokazuje

Nič fyzické. Žiarovka, iPad ani Home Assistant Green v ňom nie sú a nahradiť sa
nedajú — to je **ELYSIUM-350**, ktorý týmto zostáva otvorený a gatuje
ELYSIUM-358, 359, 360, 361 aj 362.

Konkrétne: Home Assistanta hrá `tests/ui-acceptance/stub_ha.py`, server hovoriaci
jeho WebSocket protokolom. Preto sa dá otestovať výpadok, obnova aj korelácia
`context.id` — ale „žiarovka svieti" tu znamená „stav v Home Assistante je `on`",
nie že niekto videl svetlo.

## Podrobnosti

### 1. Panel na `/`, API na `/v1`, health na `/health`

**PASS**

* panel sa na `/` nakreslil: sekcie ZARIADENIA, PREVÁDZKA, SPOJENIE
* bez chýb v konzole, a to s odrezaným internetom (zablokované: nič sa nepokúsilo)
* panel otvorený z jednotky si adresu predvyplnil sám na "http://127.0.0.1:18080/" — človek ju nemusí hľadať
* /health → 200 ok (genesis-core 0.1.0)
* /v1/me bez tokenu → 401
* chybná API cesta → 404 a nie HTML (telo 0 B, typ žiadny)

### 2. Owner/member/guest, odmietnutie guest zápisu a cudzej domácnosti

**PASS**

* panel s owner tokenom hlási "Rola: owner" a identitu pilot-owner (owner), ktorú určila jednotka
* panel s member tokenom hlási "Rola: member" a identitu pilot-member (member), ktorú určila jednotka
* panel hlási "Rola: guest" a gosťovi kartu na povel nedá — píše, že rozhoduje jednotka, nie panel
* guest klikol na prepínač zariadenia (3 na obrazovke) a v ledgeri nepribudol žiaden povel
* guest POST /v1/commands priamo na API → 403
* owner na cudziu domácnosť → 403

### 3. Párovanie kódom, uplatnenie a odobranie

**PASS**

* owner vydal kód pre pilot-tester-muqtl7u8 a panel ho zobrazil (64 znakov, tu neuvádzam)
* panel hovorí, že kód uvidí len raz
* po potvrdení „Mám ho" kód z obrazovky zmizol
* člen kód uplatnil z panela a jednotka mu vydala identitu s rolou member
* ten istý kód druhýkrát → 422, kód je jednorazový
* panel vlastníka ukazuje vydanú identitu pilot-tester-muqtl7u8
* po „Odobrať" identita už nie je platná

### 4. Inventár, miestnosti, výpadok Home Assistanta a obnova spojenia

**PASS**

* panel ukazuje „Obývačka — strop"
* panel ukazuje „Chodba"
* panel ukazuje „Bojler"
* miestnosť priradená priamo entite prebila miestnosť zariadenia (Chodba, nie Technická miestnosť)
* nepodporovaná doména (sensor.teplota) sa v inventári neobjavila
* po skutočnom vypnutí Home Assistanta linka prešla z "connected" na "disconnected"
* panel počas výpadku netvrdí, že je Home Assistant spojený
* po zapnutí Home Assistanta sa jednotka pripojila sama, bez reštartu
* po obnove vidí panel znova všetky tri zariadenia online

### 5. Zapnutie/vypnutie z panela, ledger a neistý výsledok

**PASS**

* povel z panela: stav device_confirmed, reťaz (bez histórie v odpovedi)
* ledger došiel až po device_confirmed, teda potvrdené pozorovaním stavu, nie len prijatím povelu
* pri nepotvrdenom stave ledger skončil na unknown, nie na device_confirmed
* panel neistý výsledok priznáva na obrazovke

### 6. Behavior grant, expiry a relock

**PASS**

* rozhodnutie bez podpisu → 401: kanál nepustí nepodpísané
* podpísané rozhodnutie → 200, výsledok "granted" s dôvodom "pilot_acceptance"
* grant je active a odomknutie je potvrdené pozorovaním zariadenia, nie len prijatím povelu
* panel má sekciu Časový prístup
* po uplynutí okna grant sám od seba nezmizol — jednotka ho drží, kým sa zosúladenie nevykoná
* panel ponúkol vlastníkovi „Zosúladiť teraz" na grant, ktorému uplynulo okno
* po zosúladení z panela je grant "relocked" — zamknuté späť
* po zosúladení nezostal otvorený žiaden grant s uplynutým oknom
* logický stav grantu a fyzický stav žiarovky sú oddelené v ledgeri; fyzickú žiarovku nikto nevidel (ELYSIUM-350)

### 7. Hlasový povel, nejednoznačnosť, potvrdenie citlivej akcie a audit

**PASS**

* jednoznačný povel „zapni svetlo v obývačke" panel vykonal
* nejednoznačný povel „zapni svetlo" panel neuhádol — ukázal, medzi čím sa nerozhodol
* citlivé zariadenie (bojler) si vyžiadalo potvrdenie a panel ponúkol „Potvrdiť akciu"
* po potvrdení sa akcia vykonala
* audit hlasu má 4 záznamov a každý nesie rozhodnutie aj dôvod: executed/device_confirmed, awaiting_confirmation/confirmation_required, refused/several_matching_devices, refused/no_matching_device
* v audite nie je ani jeden prepis — jednotka si nechala rozhodnutie a dôvod, nie slová

### 8. Prevádzkové centrum, záloha vlastníkom a zákaz pre člena a gosťa

**PASS**

* panel má prevádzkové centrum
* prevádzkové centrum ukazuje všetkých päť stavov zvlášť: Genesis jednotka, Home Assistant, Inventár, Otvorené incidenty, Posledná záloha
* vlastník vytvoril zálohu z panela; jednotka ich eviduje 1, prvá má 151552 B z 2026-10-02T10:30:58.687Z
* na disku jednotky skutočne je 1 súbor(ov) zálohy
* member POST /v1/backup → 403
* guest POST /v1/backup → 403
* člen tlačidlo na zálohu v paneli vôbec nevidí
* panel nesie runbook obnovy a rollbacku

### 9. Reštart aplikácie aj jednotky bez straty otvorených incidentov

**PASS**

* pred reštartom: 2 grantov (1 otvorených, ten nový je "active"), 6 povelov v ledgeri
* po znovunačítaní panela sa aplikácia nakreslila celá
* jednotka bola skutočne zastavená (SIGTERM) a spustená znova
* po reštarte jednotky: 2 grantov (1 otvorených, ten istý je "active"), 6 povelov
* reštart nezahodil ani jeden povel ani grant
* otvorený incident reštart prežil a zostal v stave "active"
* panel po reštarte jednotky znovu načítal stav a incident je v ňom vidieť

