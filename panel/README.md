# Genesis panel

Jeden Flutter/Dart projekt pre iPadOS a web. Panel má responzívnu navigáciu domácnosť/miestnosť, kontrolu `GET /health`, inventár z `GET /v1/devices` a pilotný povel cez `POST /v1/commands`. Pilotné miestnosti sú ukážkové; zariadenia sa zatiaľ zobrazujú za celú domácnosť.

## Stav povelu, ledger a diagnostika

Panel číta detail povelu z `GET /v1/commands/{command_id}` a rozlišuje všetkých šesť stavov ledgeru po jednom — `accepted`, `sent`, `provider_confirmed`, `device_confirmed`, `unknown`, `failed` — s časom zmeny a s dôkazom, keď nejaký je. `provider_ack` a `device_observation` sa nezlievajú do jednej vety: iba to druhé hovorí o fyzickom svete. Stav, ktorý panel nepozná, sa nikdy nezobrazí ako úspech.

**Neistý výsledok sa neponúka na zopakovanie.** Kým je posledný povel na zariadení `unknown`, prepínač je zamknutý a panel k tomu uvedie referenciu na nahlásenie (`correlation_id`). Jediná ponúknutá akcia je **Obnoviť stav**, čo je čítanie z ledgeru, nie druhé odoslanie. Pre panelové povely nevzniká záznam v tabuľke `incidents` — tá patrí zosúlaďovaniu grantov — takže referenciou je `correlation_id`, ktoré prechádza každým záznamom povelu v ledgeri.

Sekcia **Ledger** ukazuje posledné povely z `GET /v1/commands`, aby sa neistý povel dal nájsť aj po obnovení stránky, keď už jeho `command_id` nikto nemá.

Sekcia **Diagnostika** ukazuje stav jednotky a stav prepojenia Genesis↔Home Assistant **oddelene**, z `GET /v1/diagnostics`. Zelená jednotka neznamená, že sa inventár hýbe: `/health` odpovedá aj vtedy, keď WebSocket sedenie k Home Assistantovi spadlo. Nenastavené prepojenie sa zobrazuje ako vlastný stav, nie ako porucha.

## Vývoj

Vyžaduje Flutter SDK a pre iPadOS build Xcode na macOS. V adresári `panel/`:

```sh
flutter create --platforms=ios,web --org com.elysium.genesis .
flutter pub get
flutter run -d chrome --dart-define=GENESIS_API_URL=http://localhost:8765
flutter build web --release
flutter build ios --simulator --no-codesign
```

Webový panel v Home Assistant app verzie `0.1.2` sa otvára cez HA Ingress a automaticky použije rovnakú adresu pre API. Na iPade ako samostatnej natívnej aplikácii treba adresu Genesis API určiť podľa plánovaného spôsobu bezpečného prístupu; port 8765 sa v HA app už nepublikuje. Stav **Online** znamená úspešné volanie health endpointu, nepotvrdzuje HA inventár ani fyzické zariadenia. Prístupový token sa drží iba v pamäti otvoreného panelu a zobrazuje sa ako heslo. Panel nevydáva `provider_confirmed` za zmenu fyzického zariadenia; neistý výsledok označí `unknown`. Prepínač sa deaktivuje pri nedostupnom HA spojení, neznámom stave a tiež vtedy, keď posledný povel na tom zariadení skončil neisto. HA `last_updated` je čas poslednej zmeny zariadenia, nie lehota platnosti stavu; panel ho zobrazuje iba ako informáciu. Povel naslepo znova neodosielajte — panel to pri neistom výsledku ani neponúkne a namiesto toho ponúka prečítanie stavu z ledgeru.

Webový build potrebuje rovnaký pôvod ako API alebo reverzný proxy, ktorý rieši CORS a bezpečné HTTPS. Priame načítanie z inej webovej domény nie je v tejto etape podporované. Fyzický iPad, reálny webový prehliadač s proxy a Smart Bulb treba otestovať pri záverečnom pilotnom nasadení.

CI generuje platformové šablóny cez `flutter create` a overuje webový release build aj iOS simulator build. To nie je test na fyzickom iPade. Existujúca Elysium aplikácia v Swifte zostáva samostatná.
