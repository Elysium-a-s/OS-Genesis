# Genesis panel

Jeden Flutter/Dart projekt pre iPadOS a web. Základný panel má responzívnu navigáciu domácnosť/miestnosť a kontrolu spojenia s Genesis `GET /health`. Pilotné miestnosti sú ukážkové; zariadenia a ich ovládanie pribudnú v ďalších Jira úlohách.

## Vývoj

Vyžaduje Flutter SDK a pre iPadOS build Xcode na macOS. V adresári `panel/`:

```sh
flutter create --platforms=ios,web --org com.elysium.genesis .
flutter pub get
flutter run -d chrome --dart-define=GENESIS_API_URL=http://localhost:8765
flutter build web --release
flutter build ios --simulator --no-codesign
```

Na iPade nastavte v paneli adresu Home Assistant Green v domácej sieti, napríklad `http://<IP-Green>:8765`. Webový build potrebuje pri samostatnej doméne povolený CORS alebo proxy na rovnakom pôvode; Genesis core to zatiaľ neposkytuje. Stav **Online** znamená úspešné volanie health endpointu, nepotvrdzuje HA inventár ani fyzické zariadenia.

CI generuje platformové šablóny cez `flutter create` a overuje webový release build aj iOS simulator build. To nie je test na fyzickom iPade. Existujúca Elysium aplikácia v Swifte zostáva samostatná.
