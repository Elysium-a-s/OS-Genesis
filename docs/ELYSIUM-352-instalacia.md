# ELYSIUM-352 — inštalácia, aktualizácia a rollback HA app

**Stav:** distribučná cesta zvolená a postup zapísaný. Inštalácia na fyzickom Home Assistant Green **overená nie je** — to je ELYSIUM-350.  
**Dátum:** 2026-09-30  
**Vlastník:** OS Genesis  
**Jira:** https://sarockylukas.atlassian.net/browse/ELYSIUM-352

Nadväzuje na [ELYSIUM-348 — záloha, aktualizácia a obnova](ELYSIUM-348-obnova.md), ktorá rieši databázu. Tento dokument rieši **samotnú app**: ako sa dostane do Home Assistanta, ako sa aktualizuje a ako sa vráti späť.

## 1. Zvolená distribučná cesta

**Repozitár aj GHCR balík `os-genesis-pilot` sú verejne čitateľné.** To je to rozhodnutie; alternatíva bola nechať balík súkromný a dať Supervisoru prihlasovacie údaje k `ghcr.io` (Supervisor to vie, drží si ich per registry — `POST /docker/registry`). Verejné čítanie vyhralo, lebo nevyžaduje, aby na jednotke ležal token k registru.

Čo to znamená prakticky:

| Vec | Verejné? |
| --- | --- |
| Manifest app (`ha-app/genesis/config.yaml`) | áno |
| Obraz `ghcr.io/elysium-a-s/os-genesis-pilot` | áno, na čítanie |
| `read_token`, `write_token`, HA token | **nie** — nikdy nie sú v obraze ani v manifeste, zadáva ich prevádzkovateľ v nastaveniach app |
| Obsah ledgeru (`/data/genesis-ledger.sqlite3`) | **nie** — vzniká až na jednotke |

Inými slovami: verejný je recept, nie domácnosť.

## 2. Inštalácia

1. Home Assistant → Settings → Add-ons → **Add-on Store** → ⋮ → **Repositories** → pridať `https://github.com/Elysium-a-s/OS-Genesis`.
2. Nainštalovať **OS Genesis Pilot**.
3. V nastaveniach app vyplniť `read_token` a `write_token` — dva **rôzne** náhodné reťazce, každý aspoň 32 znakov. `member_token` a `guest_token` môžu zostať prázdne. Tokeny nikam inam neprepisujte a nedávajte ich do Gitu ani do logov.
4. Spustiť app, potom **OPEN WEB UI**. Panel ide cez HA Ingress, takže prihlásenie rieši Home Assistant a panel si adresu API doplní sám.
5. V paneli zadať `write_token` a načítať inventár. V logu app hľadať `Home Assistant inventory loaded`.

Supervisor add-on v repozitári nájde aj keď nie je v koreňovom priečinku — hľadá rekurzívne (`path.glob("**/config.*")` v `supervisor/store/data.py`), takže `ha-app/genesis/config.yaml` je v poriadku.

## 3. Aktualizácia

Tu je vec, ktorá prekvapí každého prvýkrát.

**Supervisor sa o novej verzii nedozvie z registra.** Porovnáva `version` z `config.yaml` v **repozitári** oproti nainštalovanej verzii. Repozitár si ťahá `git fetch --depth 1` (`supervisor/store/git.py`) a robí to **na vlastnom cykle — raz za 3 hodiny** (`RUN_RELOAD_APPS = 10800` v `supervisor/misc/tasks.py`).

Čiže hneď po vydaní novú verziu v HA nevidíte, a nie je to chyba. Vynútiť sa dá:

- **V UI:** Settings → Add-ons → Add-on Store → ⋮ → **Check for updates**.
- **Cez API:** `POST /store/reload`.

Potom app ukáže novú verziu a tlačidlo aktualizácie.

Po aktualizácii dajte v prehliadači tvrdé obnovenie panelu. Webové assety sa cachujú a starý panel by inak zostal pred očami aj s novým obrazom.

### Čo aktualizácia nezmaže

Schéma sa rozširuje prírastkovo a žiadna migrácia riadky neodstraňuje, takže granty, kreditívy ani pravidlá sa pri aktualizácii nestratia. Detaily sú v [ELYSIUM-348](ELYSIUM-348-obnova.md).

## 4. Rollback

Rollback app má **dve časti a poradie je dôležité.**

Staršia verzia Genesis **odmietne otvoriť novšiu databázu** (`the database was written by a newer Genesis`) — je to zámerná poistka z ELYSIUM-348, aby staršia verzia nepracovala s obsahom, o ktorom nevie. Preto samotné preinštalovanie staršej app nestačí: bez zálohy spravenej **pred** aktualizáciou sa starší obraz nenaštartuje.

Postup:

1. Zastaviť app.
2. Obnoviť zálohu databázy spravenú pred aktualizáciou — podľa postupu v [ELYSIUM-348](ELYSIUM-348-obnova.md).
3. Nainštalovať konkrétnu staršiu verziu. Supervisor to vie cez `POST /store/addons/{app}/install/{version}`; v UI sa výber verzie bežne neponúka, takže ide o zásah cez API alebo CLI, ktoré ten endpoint volá.
4. Spustiť app a skontrolovať log a `GET /health`.

Preto platí to, čo hovorí aj 348: **pred aktualizáciou si spravte zálohu.** Nie preto, že by migrácia mazala, ale preto, že bez nej sa nedá ísť späť.

## 5. Ako vzniká vydanie

Toto je strana vývoja, nie prevádzky, ale patrí sem, lebo vysvetľuje čísla verzií.

Obraz publikuje CI pri pushi do `main`, a publikuje presne ten, ktorý prešiel smoke testom. Vydávajú sa dve značky:

- `sha-<commit>` — nemenná, pri každom pushi; slúži na dohľadanie, čo presne beží,
- `<version>` — vydanie, a **nikdy sa neprepisuje**.

Meno obrazu aj verziu si workflow číta z `config.yaml`, z tej istej hodnoty sa plní `io.hass.version` v obraze a CI overuje, že **značka a label hovoria to isté**. Vydať novú verziu preto znamená zvýšiť `version` v `ha-app/genesis/config.yaml`; bez toho workflow značku neposunie a napíše varovanie.

Dôvod tej neprepisovateľnosti je priamy: Supervisor rozhoduje o aktualizácii podľa čísla verzie. Keby sa pod tým istým číslom vymenil obsah, jednotka by si myslela, že je aktuálna, a opravu by nikdy nedostala.

Doteraz vydané:

| Verzia | Digest | Obsahuje |
| --- | --- | --- |
| `0.1.2` | `sha256:1ef49e43…` | Ingress, panel v pôvodnom Material vzhľade |
| `0.1.3` | `sha256:1e049fc2…` | panel v Elysium dizajne |
| `0.1.4` | doplní sa po publikovaní | ELYSIUM-353–357 a zapnutie zálohy, citlivých zariadení a Behavior kanála v app |

Digest sa dá doplniť až po publikovaní: obraz vzniká pri pushi do `main`, takže v čase, keď sa `version` zvyšuje, ešte neexistuje. Riadok s dopísaným digestom je záznam o tom, čo bolo vydané, nie plán.

### Prázdna možnosť nie je nastavená možnosť

Voliteľné možnosti app (`sensitive_devices`, `behavior_key_id`, `behavior_secret`, `central_unit_id`) sa pri prázdnej hodnote **neexportujú vôbec**. Core ich číta cez `env::var(...).ok()`, takže prázdny reťazec preň nie je „nenastavené", ale nastavená prázdna hodnota — a to má dva konkrétne následky, ktoré by sa inak objavili až na jednotke: prázdny `GENESIS_BEHAVIOR_KEY_ID` zhodí štart app, a prázdny `GENESIS_CENTRAL_UNIT_ID` spôsobí, že jednotka odmietne každé rozhodnutie Behavior, pretože žiadne UUID sa nerovná prázdnemu reťazcu.

Platí to od `0.1.4`. V `0.1.3` tieto premenné `run.sh` nenastavoval vôbec, takže záloha, citlivé zariadenia a Behavior kanál boli v obraze, ale na jednotke sa nedali zapnúť.

## 6. Čo nie je overené

**Inštalácia na fyzickom Home Assistant Green.** Nikto ju nevykonal. Postup vyššie je odvodený zo správania Supervisora prečítaného z jeho zdrojového kódu a z toho, čo robí naše CI — nie z behu na hardvéri. To patrí k ELYSIUM-350 a kým to neprebehne, inštalovateľnosť je návrh, nie fakt.

**Rollback ako celok.** Jednotlivé časti overené sú — staršia verzia novšiu databázu naozaj odmietne (test v `core/src/ledger.rs`) a záloha sa dá otvoriť ako databáza (test v `core/src/backup.rs`) — ale celý postup za sebou na jednotke nikto neprešiel.

**Panel na iPade.** Ani na skutočnom Green, ani na fyzickom iPade ho zatiaľ nikto nevidel.
