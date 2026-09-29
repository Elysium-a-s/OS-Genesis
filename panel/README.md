# Genesis panel

Jeden Flutter/Dart projekt pre iPadOS a web. Panel má responzívnu navigáciu domácnosť/miestnosť, kontrolu `GET /health`, inventár z `GET /v1/devices` a pilotný povel cez `POST /v1/commands`. Pilotné miestnosti sú ukážkové; zariadenia sa zatiaľ zobrazujú za celú domácnosť.

## Vývoj

Vyžaduje Flutter SDK a pre iPadOS build Xcode na macOS. V adresári `panel/`:

```sh
flutter create --platforms=ios,web --org com.elysium.genesis .
flutter pub get
flutter run -d chrome --dart-define=GENESIS_API_URL=http://localhost:8765
flutter build web --release
flutter build ios --simulator --no-codesign
```

Na iPade nastavte v paneli adresu Home Assistant Green v domácej sieti, napríklad `http://<IP-Green>:8765`. Webový build potrebuje pri samostatnej doméne povolený CORS alebo proxy na rovnakom pôvode; Genesis core to zatiaľ neposkytuje. Stav **Online** znamená úspešné volanie health endpointu, nepotvrdzuje HA inventár ani fyzické zariadenia. Read a write token sa zadávajú zvlášť, držia sa iba v pamäti otvoreného panelu a zobrazujú sa ako heslá. Panel nevydáva `provider_confirmed` za zmenu fyzického zariadenia; neistý výsledok označí `unknown`. Prepínač sa deaktivuje pri nedostupnom HA spojení alebo neznámom stave. HA `last_updated` je čas poslednej zmeny zariadenia, nie lehota platnosti stavu; panel ho zobrazuje iba ako informáciu. Pri sieťovej chybe po POST neodosielajte povel naslepo znova; skontrolujte ledger.

Webový build potrebuje rovnaký pôvod ako API alebo reverzný proxy, ktorý rieši CORS a bezpečné HTTPS. Priame načítanie z inej webovej domény nie je v tejto etape podporované. Fyzický iPad, reálny webový prehliadač s proxy a Smart Bulb treba otestovať pri záverečnom pilotnom nasadení.

CI generuje platformové šablóny cez `flutter create` a overuje webový release build aj iOS simulator build. To nie je test na fyzickom iPade. Existujúca Elysium aplikácia v Swifte zostáva samostatná.
