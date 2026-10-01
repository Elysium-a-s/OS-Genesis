import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:genesis_panel/main.dart';

/// Čo nesmie byť v nainštalovanej aplikácii.
///
/// Tiket to žiada výslovne a je to to najľahšie na zabudnutie: vývojová adresa
/// alebo zabudnutý token neprekáža, kým sa buduje web do Ingressu, a zrazu je to
/// aplikácia na cudzom iPade.
void main() {
  /// Predtým tu bolo `http://localhost:8765`. Na iPade je to sám iPad, takže to
  /// nebolo len neužitočné — bola to testovacia adresa zabudnutá v produkčnom
  /// builde.
  test('the installed app ships no address of its own', () {
    // Test beží mimo webu, teda tou istou vetvou ako aplikácia na zariadení.
    expect(kIsWeb, isFalse);
    expect(genesisDefaultApiUrl(), isEmpty);
  });

  /// Zdrojové súbory nesmú nič také nosiť. Je to hrubá kontrola, ale presne tá,
  /// ktorú nikto neurobí ručne pri štvrtej zmene v `main.dart`.
  test('no source file carries a baked address, token or debug switch', () {
    final offences = <String>[];
    // Hľadá sa **cieľová adresa**, nie slovo. `lib/transport.dart` musí lokálne
    // rozsahy pomenovať, pretože ich triedi — `'localhost'` ako porovnávaný
    // reťazec nie je zapečený cieľ, kým `'http://localhost:8765'` ním bol.
    // Širší vzor by nútil vypnúť kontrolu pre celý súbor, čo je horšie: potom by
    // sa v ňom skutočná adresa schovala.
    final suspicious = <RegExp, String>{
      RegExp(r'https?://localhost'): 'vývojová adresa',
      RegExp(r'https?://127\.0\.0\.1'): 'vývojová adresa',
      RegExp(r'https?://10\.0\.2\.2'): 'adresa emulátora',
      RegExp(r'''(?:Bearer|token)\s*[:=]\s*['"][A-Za-z0-9_\-]{16,}['"]'''):
          'zapísaný token',
      RegExp(r'debugShowCheckedModeBanner:\s*true'): 'debug prepínač',
      RegExp(r'debugPrint\('): 'debug výpis',
    };
    for (final entity in Directory('lib').listSync(recursive: true)) {
      if (entity is! File || !entity.path.endsWith('.dart')) continue;
      final source = entity.readAsStringSync();
      for (final line in const LineSplitter().convert(source)) {
        // Komentár smie o tom hovoriť; práve tam je vysvetlené, prečo tam nič nie je.
        final code = line.trim();
        if (code.startsWith('//') || code.startsWith('///')) continue;
        for (final entry in suspicious.entries) {
          if (entry.key.hasMatch(code)) {
            offences.add('${entity.path}: ${entry.value} — $code');
          }
        }
      }
    }
    expect(offences, isEmpty, reason: offences.join('\n'));
  });

  /// Výnimka pre App Transport Security musí zostať úzka. `NSAllowsArbitraryLoads`
  /// by vypol vynucovanie TLS pre celú aplikáciu, teda aj pre čokoľvek
  /// z internetu — a jednotka je na lokálnej sieti.
  test('the iOS build keeps the ATS exception scoped to the local network', () {
    final plist = File('ios/Runner/Info.plist');
    expect(plist.existsSync(), isTrue,
        reason: 'iOS projekt je verzovaný, nie generovaný pri builde');
    final text = plist.readAsStringSync();
    // Tvrdí sa o `<key>`, nie o výskyte slova: komentár v plist-e vysvetľuje,
    // prečo tam `NSAllowsArbitraryLoads` nie je, a voľné hľadanie textu by
    // spadlo práve na tom vysvetlení.
    expect(text, contains('<key>NSAllowsLocalNetworking</key>'));
    expect(text, isNot(contains('<key>NSAllowsArbitraryLoads</key>')));
    // Bez tohto textu iOS 14+ prístup do lokálnej siete odmietne.
    expect(text, contains('<key>NSLocalNetworkUsageDescription</key>'));
  });

  /// Bundle identifier je to, podľa čoho sa aplikácia podpisuje a aktualizuje.
  /// Keby zostal predvolený z `flutter create`, dve veci by sa rozišli: čo je
  /// zdokumentované a čo sa naozaj nainštaluje.
  test('the bundle identifier is the documented one', () {
    final project = File('ios/Runner.xcodeproj/project.pbxproj');
    expect(project.existsSync(), isTrue);
    final text = project.readAsStringSync();
    expect(text, contains('PRODUCT_BUNDLE_IDENTIFIER = com.elysium.genesis.panel;'));
    expect(text, isNot(contains('com.example')));
    expect(text, isNot(contains('genesisPanel')));
  });
}
