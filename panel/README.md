# Genesis panel

Jeden Flutter/Dart projekt pre iPadOS a web. Panel má responzívnu navigáciu domácnosť/miestnosť, kontrolu `GET /health`, inventár z `GET /v1/inventory` a pilotný povel cez `POST /v1/commands`.

## Miestnosti sú skutočné

Domácnosť, miestnosti aj priradenie zariadení prichádzajú z `GET /v1/inventory`, teda z Home Assistanta. V paneli nie je napísaná žiadna miestnosť. Názov domácnosti je `location_name` z Home Assistanta; kým ho jednotka nenačíta, panel píše iba „Domácnosť" a nič konkrétne netvrdí.

Výber sa viaže na `area_id`, ktoré sa pri premenovaní miestnosti nemení. Keď vybraná miestnosť zanikne, panel sa vráti na celú domácnosť — držať výber na neexistujúcej miestnosti by znamenalo ukazovať prázdno a tvrdiť, že miestnosť existuje. Zariadenia bez priradenia majú vlastnú skupinu **Bez miestnosti**; panel pre ne miestnosť nevyrába.

Prázdny zoznam má tri rôzne príčiny a panel ich nezlieva: ešte sa nepozeral, pozrel sa a domácnosť je naozaj prázdna, alebo sa pozrieť nedá. Posledné dve vyzerajú v odpovedi rovnako, preto inventár nesie so sebou stav prepojenia. Keď sa register miestností nepodarilo prečítať celý (vyžaduje administrátorský token Home Assistanta), panel to napíše a zariadenia ukáže všetky.

## Prevádzka: čo beží, čo sa pokazilo a čo je zazálohované

Päť vecí, ktoré sa môžu pokaziť **nezávisle od seba**, a preto majú päť riadkov v jednej karte:

| riadok | zdroj | čo to nehovorí |
| --- | --- | --- |
| Genesis jednotka | `GET /health` | nič o Home Assistantovi — `/health` odpovedá aj keď WebSocket sedenie spadlo |
| Home Assistant | `GET /v1/diagnostics` | nič o tom, či je inventár aktuálny |
| Inventár | `GET /v1/inventory` | nič o incidentoch |
| Otvorené incidenty | spočítané z `GET /v1/access` | nič o tom, či existuje záloha |
| Posledná záloha | `GET /v1/backup` | nič o tom, či sa dá obnoviť |

Zliať ich do jediného „stav systému" by zahodilo presne tú informáciu, pre ktorú tam prevádzkovateľ chodí. Test to tvrdí na tom najnepríjemnejšom prípade: zdravá jednotka so spadnutým spojením na Home Assistanta.

Incidenty sa počítajú z toho, čo panel už má z `/v1/access` — vlastné API na to nie je a netreba ho vyrábať.

### Záloha

`POST /v1/backup` je `VACUUM INTO` za behu, takže výsledok je celý a platný aj vtedy, keď sa práve zapisuje — na rozdiel od skopírovania súboru. `GET /v1/backup` vracia, čo jednotka má, najnovšie prvé. Oboje smie **iba vlastník**; člen a hosť vidia vysvetlenie a panel sa o zoznam ani nepokúsi.

**Čas zálohy sa berie z mena súboru, nie z času úpravy.** Ten sa dá zmeniť kopírovaním aj `touch`-om, kým meno hovorí, kedy záloha skutočne vznikla. Meno sa skladá a rozoberá na jednom mieste, takže sa tie dve veci nemôžu rozísť, a dvojbodky sa vracajú iba do časovej časti za `T` — slepé `-` → `:` by zlomilo dátum, ktorý spojovníky nesie legitímne. Čo sa nedá prečítať ako RFC 3339, sa z prehľadu zahodí: cudzí súbor v priečinku nie je záloha a hlásiť ho ako zálohu by bolo horšie než ho nevidieť.

**Tri stavy, ktoré sa nezlievajú:** nenastavené zálohovanie (503), žiadna záloha (prázdny zoznam) a neprečítateľný zoznam. Prvé je stav jednotky, druhé stav domácnosti, tretie neznalosť. Zliať prvé dve by znamenalo tvrdiť, že zálohovanie funguje a nikto ho nepoužil.

**Existujúcu zálohu jednotka neprepíše** (409). Panel to povie ako fakt o bezpečnosti — predchádzajúca záloha zostáva neporušená — nie ako zlyhanie, pretože záloha, ktorá prepíše predchádzajúcu, je horšia než chýbajúca.

### Obnova: prečo tu tlačidlo nie je

**Obnovu panel nerobí a nebude.** Nie je to chýbajúca funkcia: služba si nedokáže bezpečne podsunúť databázu sama sebe pod otvoreným spojením, takže tlačidlo „obnoviť" by bolo operáciou, ktorá môže poškodiť práve ten stav, ktorý má zachraňovať. Postup sa robí pri zastavenej službe a je v [docs/ELYSIUM-348-obnova.md](../docs/ELYSIUM-348-obnova.md) spolu s aktualizáciou a rollbackom — a rollback nie je len o obraze, pretože staršia verzia novšiu databázu odmietne.

Reštartový test na skutočnom Home Assistant Green zostáva v ELYSIUM-350; emulovaný kontajner v CI ho nenahrádza.

## Časový prístup: granty, incidenty a relock

Sekcia **Časový prístup** číta `GET /v1/access` a stojí na jedinom rozlíšení: **evidencia na jednotke nie je stav zariadenia.** Grant môže byť `relocked` a žiarovka svietiť; môže byť `relock_pending` a byť dávno zhasnutá. Preto má každý grant dva samostatné riadky a panel ich nikdy nezlieva do jednej vety:

| riadok | čo hovorí |
| --- | --- |
| **Evidencia jednotky** | logický stav grantu a okno platnosti — čo si jednotka pamätá |
| **Fyzické potvrdenie** | posledný dôkaz, alebo priznanie, že žiadny nie je |

`provider` a `device` sa v druhom riadku nezlievajú. Potvrdenie poskytovateľom znamená, že potvrdil Home Assistant — o jeden krok ďalej od žiarovky, než sa zdá. Keď rozhodnutie vyžadovalo potvrdenie zariadením a prišlo len od poskytovateľa, panel to dopíše namiesto toho, aby to skryl za „potvrdené".

**Stav, ktorý panel nepozná, sa nikdy nezobrazí ako úspech.** Platí to rovnako ako pri stavoch povelu.

**Prázdny zoznam a neprečítaný zoznam nie sú to isté.** Tu je ten rozdiel najdrahší: „žiadny časový prístup nie je otvorený" by bola nepravda o tom, čo je práve v domácnosti odomknuté. Keď sa prehľad nepodarí prečítať, panel to povie.

**Granty vidí každá rola.** Kto v domácnosti žije, má vedieť, že sa mu niečo zamyká samo.

### Tlačidlo Zosúladiť teraz

Vlastník môže vyžiadať jeden prechod zosúladenia cez `POST /v1/access/{decision_id}/reconcile`, keď vidí otvorený incident a nechce čakať na periodický prechod. Člen a hosť vidia namiesto tlačidla vysvetlenie — jednotka by im odpovedala 403 a tlačidlo, ktoré nefunguje, je horšie než žiadne.

Tlačidlo je deaktivované, keď by zosúladenie nemalo čo robiť. `GenesisGrant.isReconcilable` sa pri tom drží tej istej podmienky ako `grant::is_reconcilable` na jednotke; test to tvrdí stavom po stave aj na hranici expirácie, pretože keby sa tie dve strany rozišli, tlačidlo by buď vyzeralo rozbito, alebo by chýbalo tam, kde sa dalo použiť.

**Platné okno sa tlačidlom neskracuje.** Grant, ktorý ešte platí, vráti `not_due` a panel k tomu napíše prečo: zatvoriť prístup pred expiráciou nie je zosúladenie, je to odobranie prístupu a to má vlastné rozhodnutie. Keby to tlačidlo dokázalo, človek, ktorý si prístup zaslúžil, by o neho prišiel jedným omylom.

**`settled` a `attempted` sa nezlievajú.** Prvé znamená dokázateľne zatvorený prístup, druhé že sa o to Genesis pokúsil a dôkaz nemá — vtedy grant zostáva `relock_pending` a incident otvorený. Zliať ich do jedného „hotovo" by znamenalo tvrdiť o fyzickom svete niečo, čo nikto nepotvrdil.

Stav sa po stlačení prekreslí z odpovede, nie z domnienky o tom, čo stlačenie spôsobilo.

## Hlasový povel a potvrdenie citlivej akcie

Sekcia **Hlasový povel** posiela prepis na `POST /v1/voice/commands` a potvrdzuje citlivú akciu cez `POST /v1/voice/confirmations`. Backend tok je z ELYSIUM-345 a 346; panel k nemu dáva rozhranie.

**Mikrofón v paneli nie je a nepredstiera sa.** Panel neberie zvuk, takže žiadne surové audio nevzniká a nie je čo ukladať. Rozpoznávanie reči cez Home Assistant Assist alebo iný výslovný adaptér je samostatný krok — tento tiket dáva bezpečnú textovú cestu k tomu istému intentu, a ten je pre pilot dôležitejší: prepis prejde tou istou cestou ako hlas, takže sa dá overiť rozhodovanie bez toho, aby niekto musel riešiť mikrofón.

**Štyri výsledky a ani jeden nie je chyba klienta.** Jednotka vracia nejednoznačný aj zamietnutý povel s HTTP 422, čo je stále odpoveď o domácnosti, nie porucha. Klient to preto nerieši ako výnimku a telo číta pri 200, 202 aj 422 — keby 422 vyhodilo chybu, panel by nemal čo povedať práve tam, kde človek potrebuje dôvod.

| výsledok | HTTP | čo to znamená |
| --- | --- | --- |
| `executed` | 200 | povel je v ledgeri; stav zariadenia platí podľa sekcie Zariadenia |
| `confirmation_required` | 202 | **nič sa nevykonalo**, čaká sa na výslovné potvrdenie |
| `unclear` | 422 | jednotka povel nepochopila, takže nevykonala nič |
| `refused` | 422 | zamietnuté z dôvodu, ktorý nesúvisí s porozumením |

`unclear` a `refused` sa zámerne nezlievajú do „nepodarilo sa". Nevykonané z opatrnosti a nevykonané pre nepochopenie sú pre človeka dve rôzne veci: prvé treba potvrdiť, druhé preformulovať. Stav, ktorý panel nepozná, sa nikdy nezobrazí ako úspech.

**Nejednoznačný povel panel nedohadne.** Keď jednotka vráti kandidátov, panel ich vypíše — povedať, medzi čím sa nerozhodla, je poctivejšie než si vybrať. Text zostane v poli, aby sa dal upraviť; odoslať to isté znova by nepomohlo.

**Citlivá akcia sa nevykoná na prvé slovo.** Panel napíše, že sa nič nestalo a zariadenie sa nepohlo, ukáže, dokedy potvrdenie platí, a až tlačidlo **Potvrdiť akciu** akciu vykoná. Identifikátor potvrdenia je jednorazové oprávnenie, takže ide v tele požiadavky, nikdy v adrese — to isté pravidlo ako pri párovacom kóde. Testy to tvrdia explicitne.

**Súhlas s uložením prepisu je vypnutý.** Prepis je obsah toho, čo niekto povedal vo svojej domácnosti; bez súhlasu si jednotka nechá iba intent a rozhodnutie. Audit funguje aj tak, pretože dokladá rozhodnutie, nie obsah — a prepis v ňom nie je ani vtedy, keď bol uložený.

**Identitu aktéra určuje jednotka podľa tokenu, nie panel.** Panel ju píše pod poľom, pretože práve to meno pôjde do auditu. Hlas nedáva viac práv než panel: ovládať smie vlastník a člen, nie hosť ani služba — hosťovi sa tlačidlo nezobrazí, lebo jednotka by mu odpovedala 403.

**Audit hlasu** ukazuje posledné rozhodnutia z `GET /v1/voice/audit`: čo sa rozhodlo, kým a prečo. Čakanie na potvrdenie je samo rozhodnutím a je v ňom tiež, aj keď sa nič nevykonalo.

## Prístup: párovanie a odobranie

Vlastník vydá párovací kód cez `POST /v1/pairings`, člen ho uplatní cez `POST /v1/pairings/redeem`, vlastník vidí vydané identity na `GET /v1/credentials` a prístup odoberie cez `DELETE /v1/credentials/{id}`. Backend tok je z ELYSIUM-347; panel k nemu dáva rozhranie.

**Kód aj token sa zobrazia práve raz.** Jednotka si z nich drží len odtlačok, takže ich nevie vydať druhýkrát ani vlastníkovi — a panel to pri zobrazení kódu napíše, aby to nebolo prekvapenie.

**Kód ide v tele požiadavky, nikdy v adrese.** To isté platí pre token: ten je v hlavičke `Authorization`. Testy to tvrdia explicitne — prejdú každú odoslanú požiadavku a overia, že v žiadnej URL kód ani token nie je.

**Vypršaný, už uplatnený a odobraný kód vyzerajú rovnako.** Jednotka medzi nimi nerozlišuje a panel to nedopĺňa: hádať, ktorý z tých troch to bol, by bola informácia o cudzej domácnosti. Panel povie jednu vetu a odkáže na vlastníka.

**Po odobraní panel prístup stratí.** Jednotka odpovie 401, klient to má vlastný typ (`GenesisUnauthorized`) a nerieši to ako chybu siete: token sa zahodí z úložiska aj z poľa a panel požiada o nové párovanie. Opakovať by nepomohlo a cyklus panelu sa na prázdny token už nepozrie.

**Rola rozhoduje.** Vydávať a odoberať smie iba vlastník; člen a hosť vidia namiesto toho vysvetlenie a panel sa o zoznam identít ani nepokúsi. Uplatniť kód smie každý — je to jediná cesta, ako sa k prístupu dostať.

### Kde token žije

`GenesisTokenStore` je rozhranie práve preto, že „bezpečné úložisko" znamená na každej platforme niečo iné:

| platforma | čo to je |
| --- | --- |
| iOS, iPadOS | Keychain — skutočné zabezpečené úložisko mimo procesu aplikácie |
| web | `flutter_secure_storage` hodnotu zašifruje, ale kľúč uloží do toho istého `localStorage` — **zabezpečené úložisko to nie je** |

Vo webe je skutočnou hranicou prihlásenie do Home Assistanta pred Ingressom a pôvod stránky, nie toto úložisko. Je to napísané takto priamo preto, že z názvu balíka by sa dalo vyčítať viac, než platí.

Ukladá sa preto, že párovací kód sa dá uplatniť raz: token iba v pamäti by znamenal, že člen po obnovení stránky o prístup prišiel a nový kód mu nemá kto vydať okrem vlastníka.

### Čo je na tom zatiaľ nepohodlné

Pri uplatnení kódu treba zadať aj identifikátor domácnosti. Jednotka síce obsluhuje jedinú domácnosť, ale `redeem` ju v tele očakáva, a člen, ktorý ešte token nemá, si ju nemá odkiaľ prečítať — všetky čítania domácnosti sú za tokenom. Vlastník ju teda posiela spolu s kódom. Dalo by sa to odstrániť tým, že by ju jednotka uvádzala neautentizovane, čo ale rozširuje to, čo sa dá o jednotke zistiť bez prístupu; to je rozhodnutie o bezpečnosti, nie o pohodlí, a nepatrí do tohto tiketu.

## Stav povelu, ledger a diagnostika

Panel číta detail povelu z `GET /v1/commands/{command_id}` a rozlišuje všetkých šesť stavov ledgeru po jednom — `accepted`, `sent`, `provider_confirmed`, `device_confirmed`, `unknown`, `failed` — s časom zmeny a s dôkazom, keď nejaký je. `provider_ack` a `device_observation` sa nezlievajú do jednej vety: iba to druhé hovorí o fyzickom svete. Stav, ktorý panel nepozná, sa nikdy nezobrazí ako úspech.

**Neistý výsledok sa neponúka na zopakovanie.** Kým je posledný povel na zariadení `unknown`, prepínač je zamknutý a panel k tomu uvedie referenciu na nahlásenie (`correlation_id`). Jediná ponúknutá akcia je **Obnoviť stav**, čo je čítanie z ledgeru, nie druhé odoslanie. Pre panelové povely nevzniká záznam v tabuľke `incidents` — tá patrí zosúlaďovaniu grantov — takže referenciou je `correlation_id`, ktoré prechádza každým záznamom povelu v ledgeri.

Sekcia **Ledger** ukazuje posledné povely z `GET /v1/commands`, aby sa neistý povel dal nájsť aj po obnovení stránky, keď už jeho `command_id` nikto nemá.

Stav jednotky a stav prepojenia Genesis↔Home Assistant sú v sekcii **Prevádzka** (nižšie) a sú tam **oddelene**, z `GET /v1/diagnostics`.

## Vývoj

Vyžaduje Flutter SDK a pre iPadOS build Xcode na macOS. V adresári `panel/`:

```sh
# iOS projekt je verzovaný (ELYSIUM-360) — negenerujte ho, prepísali by ste
# Info.plist, bundle identifier aj výnimku pre App Transport Security.
flutter create --platforms=web --org com.elysium.genesis .
flutter pub get
flutter run -d chrome --dart-define=GENESIS_API_URL=http://localhost:8765
flutter build web --release
flutter build ios --simulator --no-codesign
```

Odpovede sa dekódujú výslovne ako UTF-8 z `bodyBytes`, nie cez `response.body`: axum posiela `application/json` bez parametra `charset` a `package:http` bez neho padá na Latin-1, čo by zo slovenských názvov miestností a zariadení urobilo nečitateľnú zmes.

Webový panel v Home Assistant app verzie `0.1.2` sa otvára cez HA Ingress a automaticky použije rovnakú adresu pre API. Na iPade ako samostatnej natívnej aplikácii treba adresu Genesis API určiť podľa plánovaného spôsobu bezpečného prístupu; port 8765 sa v HA app už nepublikuje. Stav **Online** znamená úspešné volanie health endpointu, nepotvrdzuje HA inventár ani fyzické zariadenia. Prístupový token sa drží iba v pamäti otvoreného panelu a zobrazuje sa ako heslo. Panel nevydáva `provider_confirmed` za zmenu fyzického zariadenia; neistý výsledok označí `unknown`. Prepínač sa deaktivuje pri nedostupnom HA spojení, neznámom stave a tiež vtedy, keď posledný povel na tom zariadení skončil neisto. HA `last_updated` je čas poslednej zmeny zariadenia, nie lehota platnosti stavu; panel ho zobrazuje iba ako informáciu. Povel naslepo znova neodosielajte — panel to pri neistom výsledku ani neponúkne a namiesto toho ponúka prečítanie stavu z ledgeru.

## Kam sa panel smie pripojiť

Nešifrované spojenie je prijaté **len na lokálnu sieť**, a je to to isté pravidlo, ktoré vynucuje iOS cez `NSAllowsLocalNetworking`. Keby sa klient a platforma rozchádzali, jedna z nich by mlčky vyhrala a človek by nevedel ktorá.

| adresa | verdikt |
| --- | --- |
| `https://…` kdekoľvek | prijaté, šifrované |
| `http://` na loopback, privátne IPv4 rozsahy, link-local, CGNAT, `::1`, `fe80::/10`, `fc00::/7`, `*.local`, meno bez domény | prijaté, **pilotný režim** — panel to napíše |
| `http://` na čokoľvek iné | **odmietnuté** |

**Odmietnutie nie je nápis.** `GenesisAddress` je jediné miesto, kde sa o adrese rozhoduje, takže pri odmietnutej adrese neodošle nič ani pätnáctsekundový cyklus, ani tlačidlá — tie sú deaktivované. Test to tvrdí tým, že zoznam odoslaných požiadaviek je **prázdny**.

**Pri pochybnosti sa háda v prospech šifrovania.** `green.local.example.com` nie je `.local`, `192.168.1` nie je adresa a `10.0.0.1.evil.com` nie je privátny rozsah.

Pilotný režim panel pomenuje nahlas: token a povely nevychádzajú z vašej siete, ale kto v nej už je, ich vidí. Celý trust model vrátane cesty cez HA Ingress a reverzný proxy je v [docs/ELYSIUM-361-prenos.md](../docs/ELYSIUM-361-prenos.md).

Overenie certifikátu má vlastný test so **skutočným TLS serverom s vlastným podpisom** (`test/tls_test.dart`). Musí byť v samostatnom súbore bez `testWidgets`: `flutter_test` pri inicializácii widget bindingu presmeruje `HttpClient` na klienta, ktorý na všetko odpovie HTTP 400, a test by prešiel alebo padol z nesprávneho dôvodu.

## Nainštalovaná aplikácia na iPade a iPhone

Bundle identifier je `com.elysium.genesis.panel`, minimálny systém iOS/iPadOS 15.0, flavor žiadny. Celé rozhodnutie vrátane postupu podpisu a TestFlightu je v [docs/ELYSIUM-360-ios-aplikacia.md](../docs/ELYSIUM-360-ios-aplikacia.md).

**Aplikácia sa otvorí bez adresy jednotky.** Na zariadení `genesisDefaultApiUrl()` vracia prázdno — predtým vracalo `http://localhost:8765`, čo je na iPade sám iPad. Aplikácia naozaj nevie, kde jednotka je, a vymyslená adresa by len vyrobila spojenie, ktoré nikdy nenastane. Pri vývoji sa dá predvyplniť cez `--dart-define=GENESIS_API_URL=...`.

**Bez výnimky pre App Transport Security by aplikácia jednotku nedosiahla vôbec.** Jednotka hovorí HTTP na privátnej adrese a iOS nezabezpečené spojenia blokuje. V `Info.plist` je preto `NSAllowsLocalNetworking` — povolí HTTP len na lokálnu sieť a TLS pre všetko ostatné zostáva vynútené. `NSAllowsArbitraryLoads` tam **nie je** zámerne: vypol by ATS pre celú aplikáciu, teda aj pre čokoľvek z internetu. Test to tvrdí o `<key>` elemente a CI to po builde overuje aj na zostavenom `Info.plist`.

Od iOS 14 treba aj `NSLocalNetworkUsageDescription`, inak systém prístup do lokálnej siete odmietne a používateľ nemá čo povoliť.

**Podpísaný build ani overenie na fyzickom zariadení v tomto repozitári nie sú.** Prvé potrebuje Apple účet vlastníka, druhé patrí do ELYSIUM-350. Simulátor v CI nie je iPad.

Webový build potrebuje rovnaký pôvod ako API alebo reverzný proxy, ktorý rieši CORS a bezpečné HTTPS. Priame načítanie z inej webovej domény nie je v tejto etape podporované. Fyzický iPad, reálny webový prehliadač s proxy a Smart Bulb treba otestovať pri záverečnom pilotnom nasadení.

CI generuje platformové šablóny cez `flutter create` a overuje webový release build aj iOS simulator build. To nie je test na fyzickom iPade. Existujúca Elysium aplikácia v Swifte zostáva samostatná.
