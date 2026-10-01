import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:genesis_panel/main.dart';
import 'package:genesis_panel/token_store.dart';
import 'package:genesis_panel/transport.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

http.Response _ok(String body, [int status = 200]) =>
    http.Response.bytes(utf8.encode(body), status);

const _ownerToken = 'owner-token-of-at-least-32-characters';

String _principal() => jsonEncode({
      'household_id': 'pilot-home',
      'actor_id': 'pilot-owner',
      'role': 'owner',
      'can_control_devices': true,
      'credential_id': null,
    });

String _inventory() => jsonEncode({
      'household': {'household_id': 'pilot-home', 'name': 'U Kováčov'},
      'areas': const [],
      'devices': const [],
      'home_assistant': {'state': 'connected', 'rooms_incomplete': false},
    });

Future<void> _settle(WidgetTester tester) async {
  for (var round = 0; round < 8; round++) {
    await tester.pump(const Duration(milliseconds: 20));
  }
}

MockClient _unit(List<http.BaseRequest> seen) => MockClient((request) async {
      seen.add(request);
      final path = request.url.path;
      if (path.endsWith('/health')) return _ok(jsonEncode({'status': 'ok'}));
      if (path.endsWith('/v1/me')) return _ok(_principal());
      if (path.endsWith('/v1/inventory')) return _ok(_inventory());
      return _ok('[]');
    });

void main() {
  /// Pravidlo je to isté, ktoré vynucuje iOS cez `NSAllowsLocalNetworking`:
  /// HTTP len na lokálnu sieť, inak HTTPS. Tabuľka je napísaná adresami, nie
  /// priebehom, pretože každý nový rozsah musí niekto vedome zaradiť.
  test('plain HTTP is accepted only where iOS accepts it too', () {
    const localHttp = [
      'http://localhost:8080',
      'http://127.0.0.1:8080',
      'http://10.0.0.5:8080',
      'http://192.168.1.10:8080',
      'http://172.16.0.1:8080',
      'http://172.31.255.254:8080',
      'http://169.254.1.1:8080',
      'http://100.64.0.1:8080',
      'http://genesis.local:8080',
      'http://green:8080',
      'http://[::1]:8080',
      'http://[fe80::1]:8080',
      'http://[fd00::1]:8080',
    ];
    for (final raw in localHttp) {
      final address = GenesisAddress.parse(raw);
      expect(address.verdict, GenesisAddressVerdict.localPilot,
          reason: '$raw should be the local pilot');
      expect(address.isUsable, isTrue);
      expect(address.isLocalPilot, isTrue);
    }

    // Verejné adresy po HTTP sa odmietajú: token a povely by išli cudzími
    // sieťami čitateľne.
    const remoteHttp = [
      'http://genesis.example.com',
      'http://8.8.8.8',
      'http://172.32.0.1',        // tesne za privátnym blokom
      'http://192.169.1.1',       // tesne za 192.168/16
      'http://100.128.0.1',       // tesne za CGNAT
      'http://example.com:8080',
    ];
    for (final raw in remoteHttp) {
      final address = GenesisAddress.parse(raw);
      expect(address.verdict, GenesisAddressVerdict.insecureRemote,
          reason: '$raw should be refused');
      expect(address.isUsable, isFalse);
      expect(address.uri, isNull);
      expect(address.explanation, contains('odmieta'));
    }

    // HTTPS je prijaté kdekoľvek a nie je to pilotný režim.
    for (final raw in ['https://genesis.example.com', 'https://192.168.1.10']) {
      final address = GenesisAddress.parse(raw);
      expect(address.verdict, GenesisAddressVerdict.secure);
      expect(address.isLocalPilot, isFalse);
    }

    // Čo sa nedá použiť, sa nedá použiť.
    expect(GenesisAddress.parse('').verdict, GenesisAddressVerdict.unusable);
    expect(GenesisAddress.parse('   ').verdict, GenesisAddressVerdict.unusable);
    expect(GenesisAddress.parse('green.local').verdict,
        GenesisAddressVerdict.unusable);
    expect(GenesisAddress.parse('ws://green.local').verdict,
        GenesisAddressVerdict.unsupportedScheme);
    // `file:///…` nemá hostiteľa, takže sa zastaví skôr než na schéme. Verdikt
    // `unusable` je o tom pravda — nie je kam sa pripojiť.
    expect(GenesisAddress.parse('file:///etc/passwd').verdict,
        GenesisAddressVerdict.unusable);
  });

  /// Pri adrese sa háda v prospech šifrovania. Čo sa nedá zaradiť, lokálne nie je.
  test('an address that cannot be classified is not treated as local', () {
    expect(GenesisAddress.isLocalHost('192.168.1.1'), isTrue);
    expect(GenesisAddress.isLocalHost('192.168.1'), isFalse);
    expect(GenesisAddress.isLocalHost('192.168.1.256'), isFalse);
    expect(GenesisAddress.isLocalHost('10.0.0.1.evil.com'), isFalse);
    expect(GenesisAddress.isLocalHost('notlocal.example'), isFalse);
    // `.local` sa nesmie dať predstierať ako subdoména cudzej domény.
    expect(GenesisAddress.isLocalHost('green.local.example.com'), isFalse);
  });

  /// Odmietnutá adresa nesmie byť len nápis. Panel na ňu nesmie poslať nič.
  testWidgets('a refused address sends no request at all', (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(seen),
      tokenStore: InMemoryTokenStore(),
    ));
    await tester.enterText(
      find.widgetWithText(TextField, 'Adresa Genesis API'),
      'http://genesis.example.com',
    );
    await tester.enterText(
      find.widgetWithText(TextField, 'Prístupový token'),
      _ownerToken,
    );
    await tester.pump(const Duration(seconds: 15));
    await _settle(tester);

    expect(find.textContaining('panel odmieta'), findsOneWidget);
    // Ani cyklus panelu, ani tlačidlá: na túto adresu nešlo nič.
    expect(seen, isEmpty);
    final check = tester.widget<FilledButton>(
      find.widgetWithText(FilledButton, 'Skontrolovať spojenie'),
    );
    expect(check.onPressed, isNull);
  });

  /// Lokálny pilot funguje a panel o ňom nemlčí.
  testWidgets('the local pilot works and is named as unencrypted',
      (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(seen),
      tokenStore: InMemoryTokenStore(),
    ));
    await tester.enterText(
      find.widgetWithText(TextField, 'Adresa Genesis API'),
      'http://192.168.1.10:8080',
    );
    await tester.enterText(
      find.widgetWithText(TextField, 'Prístupový token'),
      _ownerToken,
    );
    await tester.pump(const Duration(seconds: 15));
    await _settle(tester);

    expect(find.textContaining('Pilotný režim'), findsOneWidget);
    expect(find.textContaining('nešifrované spojenie'), findsOneWidget);
    expect(seen, isNotEmpty);
    expect(find.text('U Kováčov'), findsWidgets);
  });

  /// Token nesmie byť v adrese ani v query parametri na žiadnej ceste.
  testWidgets('no request carries the token in the URL', (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(seen),
      tokenStore: InMemoryTokenStore(),
    ));
    await tester.enterText(
      find.widgetWithText(TextField, 'Adresa Genesis API'),
      'https://genesis.local',
    );
    await tester.enterText(
      find.widgetWithText(TextField, 'Prístupový token'),
      _ownerToken,
    );
    await tester.pump(const Duration(seconds: 15));
    await _settle(tester);

    expect(seen, isNotEmpty);
    for (final request in seen) {
      expect(request.url.toString(), isNot(contains(_ownerToken)));
      expect(request.url.queryParameters.values, isNot(contains(_ownerToken)));
      // Token patrí do hlavičky, nikam inam.
      for (final value in request.url.queryParameters.values) {
        expect(value.length, lessThan(32),
            reason: 'query parameter looks like a secret: $value');
      }
    }
  });
}
