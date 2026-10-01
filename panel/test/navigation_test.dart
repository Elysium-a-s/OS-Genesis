import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:genesis_panel/main.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

/// Odpoveď tak, ako ju posiela jednotka: JSON v UTF-8 bez `charset` v hlavičke.
/// `http.Response` so stringom by telo kódoval Latin-1 a na prvom „č" by spadol,
/// a presne preto panel číta bajty a nie `response.body`.
http.Response _ok(String body, [int status = 200]) =>
    http.Response.bytes(utf8.encode(body), status);

Map<String, dynamic> _light(String entity, String name, String? area) => {
      'device_id': 'ha:$entity',
      'name': name,
      'power': true,
      'availability': 'online',
      'observed_at': '2026-09-30T09:05:00Z',
      'writable': true,
      'area_id': area,
      'area_name': switch (area) {
        'ha:living_room' => 'Obývačka',
        'ha:bedroom' => 'Spálňa',
        _ => null,
      },
    };

/// Domácnosť s dvomi miestnosťami — žiadna z nich nie je v paneli napísaná —
/// a jedným zariadením, ktoré miestnosť nemá.
String _household() => jsonEncode({
      'household': {'household_id': 'pilot-home', 'name': 'U Kováčov'},
      'areas': [
        {
          'area_id': 'ha:living_room',
          'name': 'Obývačka',
          'device_ids': ['ha:light.living']
        },
        {'area_id': 'ha:bedroom', 'name': 'Spálňa', 'device_ids': []},
      ],
      'devices': [
        _light('light.living', 'Veľké svetlo', 'ha:living_room'),
        _light('switch.plug', 'Zásuvka', null),
      ],
      'home_assistant': {'state': 'connected', 'rooms_incomplete': false},
    });

Future<void> _settle(WidgetTester tester) async {
  for (var round = 0; round < 6; round++) {
    await tester.pump(const Duration(milliseconds: 20));
  }
}

MockClient _client() => MockClient((request) async {
      final path = request.url.path;
      if (path.endsWith('/health')) {
        return _ok(jsonEncode({'status': 'ok'}), 200);
      }
      if (path.endsWith('/v1/me')) {
        return _ok(
            jsonEncode({
              'household_id': 'pilot-home',
              'actor_id': 'pilot-member',
              'role': 'member',
              'can_control_devices': true,
            }),
            200);
      }
      if (path.endsWith('/v1/inventory')) {
        return _ok(_household(), 200);
      }
      if (path.endsWith('/v1/access')) return _ok('[]', 200);
      if (path.endsWith('/v1/diagnostics')) {
        return _ok(
            jsonEncode({
              'unit': {
                'version': '0.1.0',
                'household_id': 'pilot-home',
                'home_assistant_configured': true,
              },
              'home_assistant': {
                'state': 'connected',
                'since': '2026-09-30T09:00:00Z',
                'last_inventory_at': '2026-09-30T09:05:00Z',
                'last_inventory_devices': 2,
                'last_error': null,
              },
            }),
            200);
      }
      if (path.endsWith('/v1/commands')) {
        return _ok('[]', 200);
      }
      return _ok('{}', 404);
    });

void main() {
  testWidgets('rooms and the household name come from the API, not from the panel',
      (tester) async {
    tester.view.physicalSize = const Size(1200, 1600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(GenesisApp(client: _client()));
    // Pred pripojením panel netvrdí, ako sa domácnosť volá, ani aké má miestnosti.
    expect(find.text('Domácnosť'), findsWidgets);
    expect(
      find.text('Miestnosti sa načítajú z domácnosti po zadaní adresy a tokenu.'),
      findsOneWidget,
    );

    await tester.enterText(find.widgetWithText(TextField, 'Prístupový token'), 'a' * 32);
    await tester.pump(const Duration(seconds: 15));
    await _settle(tester);

    // Názov domácnosti aj miestnosti sú tie z Home Assistanta.
    expect(find.text('U Kováčov'), findsWidgets);
    expect(find.text('Obývačka'), findsWidgets);
    expect(find.text('Spálňa'), findsOneWidget);
    // Zariadenie bez priradenia má vlastnú skupinu, nie vymyslenú miestnosť.
    expect(find.text('Bez miestnosti (1)'), findsOneWidget);
    // Celá domácnosť je predvolený rozsah, takže sú vidieť obe zariadenia.
    expect(find.text('Celá domácnosť'), findsOneWidget);
    expect(find.text('Veľké svetlo'), findsOneWidget);
    expect(find.text('Zásuvka'), findsOneWidget);

    // Výber miestnosti filtruje zariadenia.
    await tester.tap(find.text('Obývačka').first);
    await _settle(tester);
    expect(find.text('Veľké svetlo'), findsOneWidget);
    expect(find.text('Zásuvka'), findsNothing);

    // Prázdna miestnosť sa neskrýva a nevydáva sa za poruchu.
    await tester.tap(find.text('Spálňa'));
    await _settle(tester);
    expect(find.text('V tejto miestnosti nie je žiadne zariadenie.'), findsOneWidget);
    expect(find.text('Veľké svetlo'), findsNothing);

    await tester.tap(find.text('Bez miestnosti (1)'));
    await _settle(tester);
    expect(find.text('Zásuvka'), findsOneWidget);
    expect(find.text('Veľké svetlo'), findsNothing);
  });
}
