# ELYSIUM-348 — záloha, aktualizácia a obnova

Postup pre prevádzkovateľa a to, čo Genesis pre obnovu robí sám. Čo z toho ešte nie je odmerané, je vymenované na konci.

## Čo je stav

Celý stav Genesis je **jeden SQLite súbor** — v Home Assistant app `/data/genesis-ledger.sqlite3`, inak podľa `GENESIS_LEDGER_PATH`. Obsahuje:

| Obsah | Prečo ho nemožno stratiť |
| --- | --- |
| Povely a ich auditné udalosti | Jediný pravdivý záznam o tom, čo sa poslalo na zariadenia |
| Časovo obmedzené granty a incidenty | Otvorený prístup, ktorý treba vrátiť späť |
| Hlasové záznamy a audit | Doklad o tom, čo asistent rozhodol a prečo |
| Vydané kreditívy | Kto má prístup; ich strata znamená nové párovanie pre všetkých |

Konfigurácia (tokeny, HA adresa) v súbore nie je — tá žije v prostredí, respektíve v možnostiach app. Záloha databázy teda **neobsahuje** bootstrap tokeny ani HA token.

## Záloha

`POST /v1/backup` s owner tokenom vypíše konzistentnú kópiu do `GENESIS_BACKUP_DIR` a vráti cestu a veľkosť. Meno nesie čas vzniku; existujúci súbor sa neprepíše.

Kópia vzniká cez SQLite `VACUUM INTO`, teda pod čítacou transakciou. **`cp` za behu nie je záloha**: môže skončiť v strede zápisu a dať poškodený súbor. Ak služba beží, používajte endpoint; ak je zastavená, obyčajná kópia súboru stačí.

Záloha je plnohodnotná databáza — dá sa otvoriť a prečítať bez obnovy, čo je najrýchlejší spôsob, ako si ju overiť. Test v `core/src/backup.rs` presne to robí.

## Obnova

Obnovu robí prevádzkovateľ pri **zastavenej službe**. Genesis ju úmyselne nevie spustiť sám: podsunúť si súbor pod otvoreným spojením je cesta k poškodeniu.

1. Zastaviť app alebo službu.
2. Odložiť si súčasný `genesis-ledger.sqlite3` (aj keď sa zdá pokazený — je to dôkazový materiál).
3. Nakopírovať zálohu na jeho miesto a odstrániť prípadné pomocné súbory (`-journal`, `-wal`, `-shm`), aby k novému súboru nezostal starý žurnál.
4. Spustiť službu a skontrolovať log: `Genesis core listening`, a pri otvorených akciách `timed access resumed after a restart` alebo `command adopted as unknown after a restart`.
5. Overiť `GET /health`, potom `GET /v1/devices` a `GET /v1/access`.

Po obnove staršej zálohy platí, že sa vrátil aj stav prístupov: kreditívy vydané po zálohe neexistujú a ich držitelia sa musia spárovať znova.

## Aktualizácia

Schéma sa rozširuje prírastkovo (`CREATE TABLE IF NOT EXISTS`, ohraničené `ALTER`), takže nová verzia si databázu doplní pri otvorení sama. Aktívne pravidlá, granty ani kreditívy sa pri tom nemažú — žiadna migrácia riadky neodstraňuje.

Databáza si pamätá verziu schémy v `PRAGMA user_version`. Pred aktualizáciou si urobte zálohu; nie preto, že by migrácia mazala, ale preto, že rollback bez zálohy je neúplný (nižšie).

## Rollback

Staršia verzia **odmietne otvoriť novšiu databázu** a skončí chybou `the database was written by a newer Genesis`. Je to zámer: keby sa spustila, pracovala by s obsahom, o ktorom nevie, a to je presne spôsob, ako sa stratia aktívne pravidlá — tichým prepísaním alebo ignorovaním.

Rollback preto znamená: zastaviť službu, obnoviť zálohu spravenú **pred** aktualizáciou, a spustiť staršiu verziu. Bez tej zálohy sa rollback nedá urobiť bezpečne.

Zábrana platí od schémy verzie 1 dopredu. Databázu, ktorá vznikla pred jej zavedením, má nulu a staršie verzie ju otvoria — pre tento jeden krok naspäť zábrana neplatí.

## Otvorené povely po reštarte

Po štarte nie je nič na ceste: povel, ktorý zostal v `accepted` alebo `sent`, už výsledok nedostane. Genesis to prizná, nedopoviedava:

1. `grant::resume` prevezme povely časovo obmedzených grantov — prizná ich ako `unknown`, založí incident `result_lost` a grant radšej považuje za otvorený, než by ho nechal bez zámku.
2. `Ledger::adopt_interrupted` doberie ostatné, teda povely z panela a z hlasu, a prizná ich ako `unknown` s dôvodom `interrupted_before_result`.

Poradie nie je ľubovoľné: keby druhý krok bežal prvý, grant by našiel povel už uzavretý a incident by nevznikol.

Žiadny z týchto povelov sa nevydáva za vykonaný. `unknown` znamená „stav zariadenia je neistý" a zosúladenie (ELYSIUM-344) ho dorovná pozorovaním alebo ďalším pokusom.

## Čo nie je odmerané

**Čas návratu.** Toto je pilotné meranie a v CI ho urobiť nemožno; potrebuje skutočný Home Assistant Green. Merať treba od zastavenia app po tri body: `GET /health` vráti 200, `GET /v1/devices` vráti aspoň jedno zariadenie (teda HA sedenie je znova nadviazané), a `GET /v1/access` nehlási nič, čo malo byť zosúladené a nebolo. Bez toho je akékoľvek číslo o čase návratu iba odhad.

**Obnova na hardvéri.** Postup vyššie je overený logicky a v testoch (záloha sa dá otvoriť, staršia verzia novšiu databázu odmietne, prerušené povely sa preberú). Na Home Assistant Green, po skutočnom výpadku napájania a s reálnym zariadením, overený nie je — patrí to k ELYSIUM-350.

**Automatická záloha.** Endpoint zálohu vytvorí, ale nikto ho sám nevolá: žiadny plán, žiadna rotácia, žiadne mazanie starých. Kým to nebude, záloha je úkon prevádzkovateľa alebo vec záloh Home Assistanta.
