import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:genesis_panel/main.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

/// Odpoveď jednotky, ktorá odpovedá na `/health` a zároveň k Home Assistantovi
/// nevidí — to je stav, v ktorom sa nesmie nič zobraziť ako úspech.
String _diagnostics(String linkState, {String? lastError}) => jsonEncode({
      'unit': {
        'version': '0.1.0',
        'household_id': 'pilot-home',
        'home_assistant_configured': true,
      },
      'home_assistant': {
        'state': linkState,
        'since': '2026-09-30T09:00:00Z',
        'last_inventory_at': null,
        'last_inventory_devices': 0,
        'last_error': lastError,
      },
    });

Map<String, dynamic> _device({
  required bool? power,
  required String availability,
}) =>
    {
      'device_id': 'ha:light.living',
      'name': 'Living',
      'power': power,
      'availability': availability,
      'observed_at': '2026-09-30T09:05:00Z',
      'writable': true,
    };

const _uncertainCommand = {
  'request': {
    'household_id': 'pilot-home',
    'command_id': 'cmd-uncertain',
    'device_id': 'ha:light.living',
    'capability_id': 'power',
    'value': true,
    'actor': {'actor_type': 'user', 'actor_id': 'pilot-member'},
    'idempotency_key': 'panel-1',
    'correlation_id': 'panel-1',
  },
  'status': 'unknown',
  // Home Assistant prijatie potvrdil predtým, než sa výsledok stratil. Dôkaz už
  // v snapshote nie je, úroveň potvrdenia áno.
  'confirmation_level': 'provider',
  'status_changed_at': '2026-09-30T09:06:00Z',
  'reason': 'no_device_confirmation',
  'evidence': null,
};

/// Necháva dobehnúť reťaz čítaní, ktorú panel spustí jedným obnovením.
Future<void> _settle(WidgetTester tester) async {
  for (var round = 0; round < 6; round++) {
    await tester.pump(const Duration(milliseconds: 20));
  }
}

void main() {
  testWidgets('an uncertain result is refreshed, never silently repeated',
      (tester) async {
    tester.view.physicalSize = const Size(1200, 1600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    var issued = 0;
    var detailReads = 0;
    final client = MockClient((request) async {
      final path = request.url.path;
      if (path.endsWith('/health')) {
        return http.Response(jsonEncode({'status': 'ok'}), 200);
      }
      if (path.endsWith('/v1/me')) {
        return http.Response(
            jsonEncode({
              'household_id': 'pilot-home',
              'actor_id': 'pilot-member',
              'role': 'member',
              'can_control_devices': true,
            }),
            200);
      }
      if (path.endsWith('/v1/devices')) {
        return http.Response(
            jsonEncode([_device(power: false, availability: 'online')]), 200);
      }
      if (path.endsWith('/v1/diagnostics')) {
        return http.Response(_diagnostics('connected'), 200);
      }
      if (path.endsWith('/v1/commands/cmd-uncertain')) {
        detailReads += 1;
        return http.Response(jsonEncode(_uncertainCommand), 200);
      }
      if (path.endsWith('/v1/commands')) {
        if (request.method == 'POST') {
          issued += 1;
          return http.Response(jsonEncode(_uncertainCommand), 200);
        }
        return http.Response(
          jsonEncode(issued == 0 ? const [] : const [_uncertainCommand]),
          200,
        );
      }
      return http.Response('{}', 404);
    });

    await tester.pumpWidget(GenesisApp(client: client));
    await tester.enterText(find.byType(TextField).at(1), 'a' * 32);
    // Panel číta sám dokola; pätnásta sekunda je jeho vlastný cyklus, takže
    // test nemusí siahať na tlačidlo mimo obrazovky.
    await tester.pump(const Duration(seconds: 15));
    await _settle(tester);

    final before = tester.widget<Switch>(find.byType(Switch));
    expect(before.onChanged, isNotNull);

    await tester.tap(find.byType(Switch));
    await _settle(tester);

    expect(issued, 1);
    expect(find.text('Neistý výsledok'), findsWidgets);
    expect(
      find.text('Nikto nevie, či sa zmena stala. Zopakovanie sa preto neponúka.'),
      findsOneWidget,
    );
    // Úroveň potvrdenia prežila, aj keď dôkaz v snapshote nie je.
    expect(
      find.text(
          'V tomto stave bez dôkazu; Home Assistant prijatie potvrdil predtým.'),
      findsOneWidget,
    );
    expect(find.text('Referencia pre nahlásenie: panel-1'), findsOneWidget);
    expect(find.text('Dôvod: no_device_confirmation'), findsOneWidget);
    // Nič sa netvári ako potvrdené.
    expect(find.text('Potvrdilo zariadenie'), findsNothing);

    // Zopakovanie sa neponúka: prepínač je zamknutý a druhý pokus o dotyk
    // nepošle nič.
    expect(tester.widget<Switch>(find.byType(Switch)).onChanged, isNull);
    await tester.tap(find.byType(Switch));
    await _settle(tester);
    expect(issued, 1);

    // Ponúka sa jediná bezpečná akcia — prečítať stav znova.
    expect(find.text('Obnoviť stav'), findsOneWidget);
    await tester.tap(find.text('Obnoviť stav'));
    await _settle(tester);
    expect(detailReads, 1);
    expect(issued, 1);
  });

  testWidgets('a dropped Home Assistant link is not hidden behind a healthy unit',
      (tester) async {
    tester.view.physicalSize = const Size(1200, 1600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final client = MockClient((request) async {
      final path = request.url.path;
      if (path.endsWith('/health')) {
        return http.Response(jsonEncode({'status': 'ok'}), 200);
      }
      if (path.endsWith('/v1/me')) {
        return http.Response(
            jsonEncode({
              'household_id': 'pilot-home',
              'actor_id': 'pilot-member',
              'role': 'member',
              'can_control_devices': true,
            }),
            200);
      }
      if (path.endsWith('/v1/devices')) {
        // Po strate sedenia jednotka prizná neznámy stav namiesto posledného.
        return http.Response(
            jsonEncode([_device(power: null, availability: 'unknown')]), 200);
      }
      if (path.endsWith('/v1/diagnostics')) {
        return http.Response(
            _diagnostics('disconnected', lastError: 'authentication'), 200);
      }
      if (path.endsWith('/v1/commands')) {
        return http.Response('[]', 200);
      }
      return http.Response('{}', 404);
    });

    await tester.pumpWidget(GenesisApp(client: client));
    await tester.enterText(find.byType(TextField).at(1), 'b' * 32);
    await tester.pump(const Duration(seconds: 15));
    await _settle(tester);

    // Jednotka odpovedá — a to je celé, čo tým hovorí.
    expect(find.text('Odpovedá'), findsOneWidget);
    expect(find.text('Online'), findsOneWidget);
    // Prepojenie je oddelene, a spadnuté.
    expect(find.text('Spojenie spadlo'), findsOneWidget);
    expect(find.text('Spojené'), findsNothing);
    expect(
      find.text('Naposledy skončilo: Home Assistant token neprijal'),
      findsOneWidget,
    );
    expect(find.text('Inventár sa v tomto behu ešte nenačítal.'), findsOneWidget);
    // Zariadenie sa netvári zapnuté ani vypnuté a ovládať sa nedá.
    expect(find.text('Stav zastaraný alebo neznámy'), findsOneWidget);
    expect(tester.widget<Switch>(find.byType(Switch)).onChanged, isNull);
  });
}
