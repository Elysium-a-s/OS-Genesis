# ELYSIUM-361 — zabezpečený prenos medzi panelom a jednotkou

Čo je šifrované, čo nie je, a prečo je to v pilote obhájiteľné len na lokálnej sieti.

## 1. Dve cesty a dva trust modely

| cesta | kto terminuje TLS | čo chráni prenos | certifikát |
| --- | --- | --- | --- |
| webový panel cez HA Ingress | Home Assistant | TLS HA + prihlásenie do HA pred Ingressom | certifikát HA (Nabu Casa, Let's Encrypt alebo vlastný) |
| nainštalovaná aplikácia → jednotka v LAN | nikto (HTTP) | hranica lokálnej siete | žiadny |
| nainštalovaná aplikácia → reverzný proxy → jednotka | proxy | TLS proxy | certifikát proxy |

**Webová cesta je už zabezpečená a nie je to naša zásluha**: panel sa podáva cez HA Ingress, takže TLS aj prihlásenie rieši Home Assistant a panel je na tom istom pôvode ako API. Genesis token je nad tým druhá vrstva — chráni povely aj pred niekým, kto sa do HA dostal.

**Priama cesta z aplikácie na jednotku je HTTP.** Jednotka má privátnu adresu a certifikát pre `192.168.x.y` nemá kto vydať; zariadenie by takému certifikátu neverilo. To je **pilotný režim**, nie cieľový stav.

## 2. Pravidlo, ktoré platí na oboch stranách

Panel prijme nešifrované spojenie **len na lokálnu sieť**:

| adresa | verdikt |
| --- | --- |
| `https://…` kdekoľvek | prijaté, šifrované |
| `http://` na loopback, `10/8`, `192.168/16`, `172.16/12`, `169.254/16`, `100.64/10`, `::1`, `fe80::/10`, `fc00::/7`, `*.local`, meno bez domény | prijaté, **pilotný režim** — panel to napíše |
| `http://` na čokoľvek iné | **odmietnuté** |
| iná schéma, adresa bez hostiteľa | nepoužiteľné |

Je to **to isté pravidlo**, ktoré vynucuje iOS cez `NSAllowsLocalNetworking` (ELYSIUM-360). Keby sa klient a platforma rozchádzali, jedna z nich by mlčky vyhrala a človek by nevedel, ktorá: na iPhone by adresa nefungovala bez vysvetlenia, alebo by panel tvrdil, že je všetko v poriadku tam, kde systém spojenie zahodil.

**Odmietnutie nie je nápis.** `GenesisAddress` je jediné miesto, kde sa rozhoduje o adrese, a `_baseUrl()` z neho vracia `null` — takže pri odmietnutej adrese neodošle nič ani pätnáctsekundový cyklus panelu, ani tlačidlá, ktoré sú deaktivované. Test to tvrdí tým, že po zadaní verejnej `http://` adresy je zoznam odoslaných požiadaviek **prázdny**.

**Pri pochybnosti sa háda v prospech šifrovania.** Čo sa nedá zaradiť ako lokálne, lokálne nie je. `green.local.example.com` nie je `.local`, `192.168.1` nie je adresa a `10.0.0.1.evil.com` nie je privátny rozsah — všetky tri sú v teste.

## 3. Tokeny

Token ide v hlavičke `Authorization`, nikdy v adrese ani v query parametri. Platí to pre každú cestu vrátane párovania a potvrdenia citlivej akcie: kód aj identifikátor potvrdenia idú v tele.

Testy to tvrdia naprieč: prechádzajú **každú** odoslanú požiadavku a overujú, že token nie je v URL ani medzi query parametrami, a že žiadny query parameter nevyzerá ako tajomstvo. `release_build_test.dart` navyše kontroluje, že v `lib/` nie je zapečená adresa, token ani debug prepínač — a v zostavenom artefakte to po builde overuje CI.

Jednotka token do logu nezapisuje; diagnostika nevydáva ani adresu, ani token prepojenia na HA.

## 4. Neplatný certifikát

`test/tls_test.dart` postaví **skutočný TLS server s vlastným podpisom** a overí, že klient spojenie nedokončí (`HandshakeException`). Potom tým istým serverom prejde klientom, ktorý certifikát ignoruje, a dostane 200 — takže test nemeria len to, že spojenie zlyhá, ale že zlyhá **na certifikáte**.

Certifikát sa generuje pri teste cez `openssl`; v repozitári neleží. Podpisový materiál do gitu nepatrí ani keď je bezcenný, a `.gitignore` to hovorí tiež. Bez `openssl` sa test preskočí s dôvodom — predstierať sa nebude.

**Tento test musí byť v samostatnom súbore bez jediného `testWidgets`.** `flutter_test` pri inicializácii widget bindingu nastaví `HttpOverrides.global` na klienta, ktorý na každú požiadavku odpovie HTTP 400, aby testy nechodili do siete. V súbore s widget testami by namiesto chyby certifikátu prišlo 400 a test by prešiel alebo padol z nesprávneho dôvodu.

## 5. Čo nie je hotové

**TLS priamo na jednotke.** Genesis sám HTTPS neponúka. Cesty, ktoré sú obhájiteľné, sú HA Ingress (hotová) alebo reverzný proxy pred jednotkou s certifikátom, ktorému zariadenie verí. Vlastná certifikačná autorita pre domácnosť by znamenala rozdávať korenný certifikát na zariadenia — to je rozhodnutie o trust modeli domácnosti, nie úloha na jeden tiket, a nepatrí sem bez toho, aby ho niekto schválil.

**Obnova po reštarte je overená v testoch panela, nie na jednotke.** Panel po odmietnutí tokenu zahodí token a požiada o nové párovanie; po reštarte jednotky sa pätnáctsekundový cyklus pripojí znova. Overené je to proti mocku a proti ledgeru v `core`, nie proti reštartu skutočnej jednotky — to patrí do **ELYSIUM-350**.

**Nikto to nespustil na Home Assistant Green.** Pravidlo o adresách je overené ako logika a ako chovanie panela, nie ako spojenie z iPadu na jednotku v domácnosti.
