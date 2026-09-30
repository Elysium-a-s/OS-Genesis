# ELYSIUM-347 — párovanie, roly a správa tajomstiev

Návrh a dôvody za tým, ako sa ku Genesis získava a odoberá prístup. Prevádzkový postup je v [core/README.md](../core/README.md#párovanie-a-prístupové-tokeny).

## Čomu má návrh zabrániť

Genesis ovláda fyzické zariadenia v cudzom dome. Z toho vyplývajú tri veci, ktorým sa treba vyhnúť:

1. **Prístup z konfigurácie.** Keď je jediným tajomstvom hodnota v premennej prostredia, prístup má každý, kto raz uvidel konfiguráciu — a nedá sa odobrať bez reštartu služby.
2. **Prístup, ktorý sa nedá zrušiť.** Stratený telefón, odídený spolubývajúci, vyzradený token. Bez revokácie je jediná odpoveď zmeniť tajomstvo všetkým naraz.
3. **Ovládanie cudzej domácnosti.** Skopírovaná databáza alebo omylom nasmerovaný klient nesmú dať vládu nad inou domácnosťou, než akú spravuje daná jednotka.

## Ako to návrh rieši

**Párovanie namiesto zdieľaného tajomstva.** Vlastník spustí párovanie a dostane jednorazový kód. Kód nie je prístup — je to oprávnenie vymeniť ho raz za prístupový token pre pomenovaného aktéra. Platí desať minút, pretože párovanie je úkon, ktorý niekto robí práve teraz, nie oprávnenie na neskôr.

**Odtlačok namiesto tajomstva.** V databáze je z kódu aj z tokenu iba SHA-256. Genesis token nedokáže zopakovať ani zobraziť; kto ho stratí, spraví nové párovanie. Výpis tabuliek nedá nikomu hodnotu, ktorou sa dá vojsť.

Zdržiavacia funkcia (argon2, bcrypt) by tu nechránila pred ničím: tokeny nie sú heslá volené človekom, sú to náhodné hodnoty s viac než dvomi stovkami bitov entropie. Slovníkový ani hrubý útok proti nim nemá o čo sa oprieť, takže cena za spomalenie by sa zaplatila bez výnosu.

**Revokácia ako prvotriedny úkon.** Vlastník vidí, čo je vydané, kedy to bolo naposledy použité, a ktorúkoľvek kreditívu odoberie jedným volaním. Odobranie je okamžité — ďalšia požiadavka s tým tokenom je 401 — a zostáva po ňom záznam, kto a kedy prístup odobral.

**Domácnosť ako súčasť kreditívy.** Kreditíva nesie domácnosť, pre ktorú bola vydaná, a jednotka prijme iba kreditívu svojej domácnosti. Prenesená databáza tak cudziu domácnosť neovládne. Nad tým zostáva pôvodná kontrola: povel s cudzím `household_id` je zamietnutý aj vtedy, keď rola na ovládanie stačí.

**Aktér namiesto roly.** Kreditíva má `actor_id`, ktorý si vlastník zvolí pri párovaní. Execution ledger, hlasový audit aj potvrdzovanie citlivých akcií tak hovoria kto, nie iba akou rolou — čo je presne to, čo ELYSIUM-346 pri zdieľaných tokenoch nemohol splniť.

## Roly

| Rola | Čítanie | Ovládanie zariadení | Párovanie a revokácia |
| --- | --- | --- | --- |
| `owner` | áno | áno | áno |
| `member` | áno | áno | nie |
| `guest` | áno | nie | nie |
| `service` | áno | nie | nie |

Vlastník smie vydať aj ďalšieho vlastníka; dvaja vlastníci jednej domácnosti sú legitímny stav, nie chyba. Roly zostávajú tie isté, aké vynucovalo ELYSIUM-341 — párovanie mení to, **komu** sa vydávajú, nie čo znamenajú.

## Čo návrh nerieši

- **Tokeny z konfigurácie zostávajú** a sú prvé v poradí overovania. Sú bootstrapom: bez nich by vlastník nemal čím spustiť prvé párovanie, a pilotný Home Assistant app ich nastavuje vo svojich možnostiach. Znamená to, že plnohodnotný prístup bez záznamu o vydaní existuje, kým sú nastavené, a odoberá sa len zmenou konfigurácie a reštartom. Odstrániť ich má zmysel až vtedy, keď párovanie prejde pilotom.
- **Vydané kreditívy nemajú expiráciu**, iba odobranie. Krátka platnosť s obnovou by bola bezpečnejšia, ale vyžaduje refresh tok na strane klienta, ktorý panel zatiaľ nemá.
- **Výmena kódu nemá rate limiting.** Pri kóde s vyše sto bitmi entropie nie je hádanie cesta. Keby sa mal kód niekedy zadávať rukou, a teda skrátiť, rate limiting sa stane podmienkou, nie zlepšením.
- **Rotácia** je dnes „vydaj nový, odober starý", nie samostatný tok.
- **Prenos tajomstva chráni len sieť.** Kód aj token idú v tele HTTP odpovede; bez TLS ich mimo dôveryhodnej LAN vidí sieť. To nie je vlastnosť tohto návrhu, ale podmienka nasadenia, ktorú `core/README.md` uvádza pre všetky endpointy.
- **Nič z toho nebolo overené na pilotnom hardvéri.** Overené je chovanie v testoch; fyzické nasadenie patrí k ELYSIUM-350.
