# ELYSIUM-360 — inštalovateľná Genesis aplikácia pre iPad a iPhone

Čo je hotové, čo je rozhodnuté a čo **nikto nemôže urobiť bez Apple účtu a zariadenia**.

## 1. iOS projekt je verzovaný

`panel/ios/` je v gite. Dovtedy ho CI generovalo cez `flutter create` pri každom builde, čo pre webový panel stačilo, ale pre inštalovateľnú aplikáciu nie: bundle identifier, `Info.plist` a výnimka pre App Transport Security sú rozhodnutia, nie generovateľné veci. Keby sa regenerovali, zmizli by bez toho, aby to čokoľvek zahlásilo.

Machine-specific časti (`Flutter/Generated.xcconfig`, `Flutter/ephemeral/`, `Pods/`, `GeneratedPluginRegistrant.*`) drží mimo gitu `panel/ios/.gitignore`, ktorý `flutter create` vygeneroval správne. CI už `flutter create --platforms=ios` nespúšťa.

## 2. Bundle identifier, podpis a flavor

| vec | hodnota | kde |
| --- | --- | --- |
| Bundle identifier | `com.elysium.genesis.panel` | `ios/Runner.xcodeproj/project.pbxproj` |
| Zobrazený názov | `Genesis Panel` | `CFBundleDisplayName` |
| Minimálny systém | iOS / iPadOS 15.0 | `IPHONEOS_DEPLOYMENT_TARGET` |
| Orientácie | iPhone bez hlavy nadol, iPad všetky štyri | `UISupportedInterfaceOrientations*` |
| Verzia a build | z `pubspec.yaml` (`version: 0.1.0+1`) | `FLUTTER_BUILD_NAME`, `FLUTTER_BUILD_NUMBER` |

Predvolený identifikátor z `flutter create` bol `com.elysium.genesis.genesisPanel`. Premenovaný je preto, že podľa bundle identifikátora sa aplikácia podpisuje a aktualizuje — a keby sa to, čo je zdokumentované, rozišlo s tým, čo sa nainštaluje, zistí sa to pri prvej aktualizácii.

**Flavor nie je.** Jeden build, jedna konfigurácia. Adresa jednotky nie je zapečená: nastavuje ju človek v aplikácii, alebo sa dá predvyplniť cez `--dart-define=GENESIS_API_URL=...` pri vývoji. Druhý flavor by znamenal druhý identifikátor a druhý podpis pre niečo, čo sa odlišuje jedným textovým poľom.

**Podpis zatiaľ nie je nastavený v projekte.** `DEVELOPMENT_TEAM` nie je vyplnený, pretože Team ID je účet vlastníka a nepatrí do repozitára predtým, než o ňom niekto rozhodne. Doplní sa pri prvom podpísanom builde (krok 5 nižšie).

## 3. Prečo by aplikácia bez tohto nefungovala vôbec

Toto je najdôležitejšia vec v celom tikete a nedalo by sa na ňu prísť inak než s iPadom v ruke.

Jednotka beží v domácnosti a hovorí **HTTP** na privátnej adrese (`http://<ip>:8080`). iOS má App Transport Security a nezabezpečené spojenia blokuje. Nainštalovaná aplikácia by sa teda na jednotku nedostala — a nie s chybou, ktorá to povie, ale ako spojenie, ktoré nikdy nenastane.

Výnimka je v `Info.plist` a je **úzka**:

```xml
<key>NSAppTransportSecurity</key>
<dict>
  <key>NSAllowsLocalNetworking</key>
  <true/>
</dict>
```

`NSAllowsLocalNetworking` povolí HTTP len na lokálnu sieť — `.local`, link-local a privátne rozsahy — a TLS pre všetko ostatné zostáva vynútené. **`NSAllowsArbitraryLoads` by vypol ATS pre celú aplikáciu**, teda aj pre čokoľvek z internetu, a to je o niekoľko rádov širšie, než táto aplikácia potrebuje. Test `release_build_test.dart` to tvrdí o `<key>` elemente a CI to po builde overuje na zostavenom `Info.plist` cez `PlistBuddy` — zdroj aj výsledok, pretože zmiznúť to môže v oboch.

Od iOS 14 systém navyše bez `NSLocalNetworkUsageDescription` prístup do lokálnej siete odmietne a používateľovi sa nezobrazí čo povoliť. Text je v plist-e a hovorí, na čo to je.

## 4. V builde nie je testovacia adresa ani token

`genesisDefaultApiUrl()` vracia na zariadení **prázdno**. Predtým vracalo `http://localhost:8765`, čo je na iPade sám iPad: testovacia adresa zabudnutá v produkčnom builde, presne to, čo akceptačné kritérium zakazuje. Aplikácia naozaj nevie, kde jednotka je, a jediná poctivá odpoveď je spýtať sa.

`release_build_test.dart` prechádza `lib/` a hľadá vývojové adresy, zapísané tokeny a debug prepínače. Je to hrubá kontrola — ale presne tá, ktorú nikto neurobí ručne pri štvrtej zmene v `main.dart`.

## 5. Čo chýba a prečo to nemôžem urobiť

**Podpísaný build a TestFlight.** Potrebuje Apple Developer účet, certifikát, provisioning profile a App Store Connect. Nemám účet vlastníka, nemám podpisovú identitu a tento kontejner nie je macOS. Toto nie je vec, ktorú by som dokončil neskôr — je to vec, ktorú musí urobiť vlastník.

Postup, keď k tomu bude prístup:

1. V App Store Connect vytvoriť App ID pre `com.elysium.genesis.panel`.
2. V Xcode otvoriť `panel/ios/Runner.xcworkspace`, v *Signing & Capabilities* zvoliť tím; tým sa vyplní `DEVELOPMENT_TEAM` — commitnúť ho.
3. `flutter build ipa --release` (voliteľne s `--dart-define=GENESIS_API_URL=...`, ak sa má adresa predvyplniť).
4. `xcrun altool`/Transporter alebo Xcode Organizer → TestFlight.
5. Interná distribúcia: pilotný tester dostane pozvánku v TestFlight, nainštaluje, zadá adresu jednotky a prístupový token z párovacieho kódu.

**Overenie na fyzickom zariadení.** iPad layout, iPhone layout, lokálna sieť a reconnect sa musia overiť na skutočnom zariadení a dôkaz patrí do **ELYSIUM-350**, ako tento tiket sám hovorí. CI stavia iba simulátor; to nie je hardware a netvrdím, že je.

## 6. Čo nie je overené

Projekt sa nezostavil na macOS mimo CI — tento kontejner Xcode nemá. `ipad-simulator` job to overuje pri každom PR, ale simulátor nie je iPad: nemá fyzickú sieť, nemá dotyk a nemá rozhodovanie systému o lokálnej sieti, ktoré sa deje až pri prvom spojení.

Výnimka pre ATS je prečítaná z Apple dokumentácie a overená ako obsah plist-u, nie ako funkčné spojenie z iPadu na jednotku. To je ten istý rozdiel ako medzi „obraz je stiahnuteľný" a „beží na Green".
