import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:genesis_panel/main.dart';
import 'package:genesis_panel/token_store.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

/// Odpoveď tak, ako ju posiela jednotka: JSON v UTF-8 bez `charset`.
http.Response _ok(String body, [int status = 200]) =>
    http.Response.bytes(utf8.encode(body), status);

const _ownerToken = 'owner-token-of-at-least-32-characters';
const _guestToken = 'guest-token-of-at-least-32-chars!!';

String _principal(String role) => jsonEncode({
      'household_id': 'pilot-home',
      'actor_id': role == 'owner' ? 'pilot-owner' : 'pilot-guest',
      'role': role,
      // Hosť zariadenia neovláda. Rozhoduje o tom jednotka, nie panel.
      'can_control_devices': role != 'guest',
      'credential_id': null,
    });

String _inventory() => jsonEncode({
      'household': {'household_id': 'pilot-home', 'name': 'U Kováčov'},
      'areas': const [],
      'devices': [
        {
          'device_id': 'ha:light.living',
          'name': 'Obývačka — strop',
          'power': false,
          'availability': 'online',
          'observed_at': '2026-10-01T09:00:00Z',
          'writable': true,
        },
        {
          'device_id': 'ha:lock.front',
          'name': 'Vchodové dvere',
          'power': true,
          'availability': 'online',
          'observed_at': '2026-10-01T09:00:00Z',
          'writable': true,
        }
      ],
      'home_assistant': {'state': 'connected', 'rooms_incomplete': false},
    });

String _diagnostics() => jsonEncode({
      'unit': {
        'version': '0.1.4',
        'household_id': 'pilot-home',
        'home_assistant_configured': true,
      },
      'home_assistant': {
        'state': 'connected',
        'since': '2026-10-01T09:00:00Z',
        'last_inventory_at': '2026-10-01T09:05:00Z',
        'last_inventory_devices': 2,
        'last_error': null,
      },
    });

/// Vykonaný povel: 200 a snapshot v ledgeri.
String _executed() => jsonEncode({
      'outcome': 'executed',
      'intent': {
        'intent': 'set_power',
        'device_id': 'ha:light.living',
        'value': true
      },
      'command': {
        'request': {
          'command_id': 'cmd:voice-1',
          'correlation_id': 'panel-voice-1',
          'device_id': 'ha:light.living',
        },
        'status': 'device_confirmed',
        'confirmation_level': 'device',
        'status_changed_at': '2026-10-01T09:10:00.000Z',
        'reason': null,
        'evidence': null,
      },
      'explanation': {
        'code': 'device_confirmed',
        'message': 'Zariadenie potvrdilo zmenu.',
      },
    });

/// Nejednoznačný povel: 422, žiadny povel, a zoznam toho, medzi čím sa
/// jednotka nerozhodla.
String _unclear() => jsonEncode({
      'outcome': 'unclear',
      'reason': 'several_matching_devices',
      'candidates': ['ha:light.living', 'ha:lock.front'],
      'message': 'Povel sa dá pochopiť viacerými spôsobmi, takže sa nevykonal.',
    });

/// Citlivá akcia: 202, nič sa nevykonalo, čaká sa na potvrdenie.
String _awaiting() => jsonEncode({
      'outcome': 'confirmation_required',
      'intent': {
        'intent': 'set_power',
        'device_id': 'ha:lock.front',
        'value': false
      },
      'confirmation_id': '7f3ad21c',
      'expires_at': '2026-10-01T09:12:00.000Z',
      'explanation': {
        'code': 'confirmation_required',
        'message': 'Citlivá akcia potrebuje potvrdenie.',
      },
    });

/// Zamietnutie: 422 s dôvodom, ktorý nesúvisí s porozumením.
String _refused(String code) => jsonEncode({
      'outcome': 'refused',
      'explanation': {
        'code': code,
        'message': 'Potvrdenie už neplatí.',
      },
    });

String _audit(List<Map<String, dynamic>> events) => jsonEncode(events);

Map<String, dynamic> _auditEvent(String decision, String reason) => {
      'event_id': 'ev-$decision',
      'household_id': 'pilot-home',
      'actor_id': 'pilot-owner',
      'decision': decision,
      'reason': reason,
      'device_id': 'ha:lock.front',
      'command_id': null,
      'at': '2026-10-01T09:11:00.000Z',
    };

Future<void> _settle(WidgetTester tester) async {
  for (var round = 0; round < 8; round++) {
    await tester.pump(const Duration(milliseconds: 20));
  }
}

MockClient _unit({
  required List<http.BaseRequest> seen,
  String role = 'owner',
  String? spoken,
  int spokenStatus = 200,
  String? confirmed,
  int confirmedStatus = 200,
  List<Map<String, dynamic>> audit = const [],
}) =>
    MockClient((request) async {
      seen.add(request);
      final path = request.url.path;
      if (path.endsWith('/health')) return _ok(jsonEncode({'status': 'ok'}));
      if (path.endsWith('/v1/voice/confirmations')) {
        return _ok(confirmed ?? _refused('unknown_confirmation'),
            confirmedStatus);
      }
      if (path.endsWith('/v1/voice/commands')) {
        return _ok(spoken ?? _executed(), spokenStatus);
      }
      if (path.endsWith('/v1/voice/audit')) return _ok(_audit(audit));
      if (path.endsWith('/v1/me')) return _ok(_principal(role));
      if (path.endsWith('/v1/inventory')) return _ok(_inventory());
      if (path.endsWith('/v1/access')) return _ok('[]');
      if (path.endsWith('/v1/diagnostics')) return _ok(_diagnostics());
      if (path.endsWith('/v1/credentials')) return _ok('[]');
      if (path.endsWith('/v1/commands')) return _ok('[]');
      return _ok('{}', 404);
    });

Future<void> _signIn(WidgetTester tester, String token) async {
  // Nainštalovaná aplikácia sa otvorí bez adresy, takže ju zadá človek —
  // test to robí rovnako ako používateľ na iPade.
  await tester.enterText(
    find.widgetWithText(TextField, 'Adresa Genesis API'),
    'http://green.local:8080',
  );
  await tester.enterText(
    find.widgetWithText(TextField, 'Prístupový token'),
    token,
  );
  await tester.pump(const Duration(seconds: 15));
  await _settle(tester);
}

Future<void> _say(WidgetTester tester, String transcript) async {
  final field = find.widgetWithText(TextField, 'Povel');
  await tester.ensureVisible(field);
  await _settle(tester);
  await tester.enterText(field, transcript);
  final button = find.widgetWithText(FilledButton, 'Odoslať povel');
  await tester.ensureVisible(button);
  await _settle(tester);
  await tester.tap(button);
  await _settle(tester);
}

void main() {
  /// Bežný povel. Vykonané je jediný stav, ktorý smie vyzerať ako hotovo.
  testWidgets('an understood command is executed and says what confirmed it',
      (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(
        seen: seen,
        spoken: _executed(),
        audit: [_auditEvent('executed', 'device_confirmed')],
      ),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _ownerToken);
    await _say(tester, 'zapni svetlo v obývačke');

    expect(find.text('Vykonané'), findsWidgets);
    // Názov z inventára, nie identifikátor.
    expect(find.text('Obývačka — strop — zapnúť'), findsOneWidget);
    expect(find.text('Dôvod: device_confirmed'), findsOneWidget);

    // Prepis ide v tele, nikdy v adrese.
    final spoken =
        seen.where((r) => r.url.path.endsWith('/v1/voice/commands')).toList();
    expect(spoken, hasLength(1));
    expect(spoken.single.method, 'POST');
    for (final request in seen) {
      expect(request.url.toString(), isNot(contains('zapni')));
      expect(request.url.toString(), isNot(contains(_ownerToken)));
    }
  });

  /// Súhlas s uložením prepisu je vypnutý, kým ho človek nezapne — prepis je
  /// obsah toho, čo niekto povedal vo svojej domácnosti.
  testWidgets('storing the transcript is off until somebody turns it on',
      (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(seen: seen, spoken: _executed()),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _ownerToken);
    await _say(tester, 'zapni svetlo v obývačke');

    final first = jsonDecode(
            (seen.firstWhere((r) => r.url.path.endsWith('/v1/voice/commands'))
                    as http.Request)
                .body)
        as Map<String, dynamic>;
    expect(first['store_transcript'], false);
    expect(first['transcript'], 'zapni svetlo v obývačke');

    // Keď súhlas dá, pošle sa true — a nie skôr.
    final consent = find.byType(Checkbox);
    await tester.ensureVisible(consent);
    await _settle(tester);
    await tester.tap(consent);
    await _settle(tester);
    await _say(tester, 'zhasni svetlo v obývačke');

    final second = jsonDecode((seen
            .lastWhere((r) => r.url.path.endsWith('/v1/voice/commands'))
        as http.Request)
            .body) as Map<String, dynamic>;
    expect(second['store_transcript'], true);
  });

  /// Nejednoznačný povel sa nevykoná a panel nehádá. Povedať, medzi čím sa
  /// jednotka nerozhodla, je jediná poctivá odpoveď.
  testWidgets('an ambiguous command executes nothing and names the candidates',
      (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(seen: seen, spoken: _unclear(), spokenStatus: 422),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _ownerToken);
    await _say(tester, 'zhasni to');

    expect(find.text('Nevykonané'), findsOneWidget);
    expect(find.text('Vykonané'), findsNothing);
    expect(find.text('Povel sa nevykonal'), findsOneWidget);
    expect(find.text('Jednotka sa nerozhodla medzi:'), findsOneWidget);
    expect(find.text('Obývačka — strop'), findsWidgets);
    expect(find.text('Vchodové dvere'), findsWidgets);

    // Text zostal v poli, aby sa dal upraviť — opakovať to isté nemá zmysel.
    expect(
      tester.widget<TextField>(find.widgetWithText(TextField, 'Povel')).controller?.text,
      'zhasni to',
    );
  });

  /// Citlivá akcia. Dve veci musia platiť naraz: nič sa nevykonalo, a potvrdiť
  /// sa to dá výslovne.
  testWidgets('a sensitive action waits for an explicit confirmation',
      (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(
        seen: seen,
        spoken: _awaiting(),
        spokenStatus: 202,
        confirmed: _executed(),
        audit: [_auditEvent('awaiting_confirmation', 'confirmation_required')],
      ),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _ownerToken);
    await _say(tester, 'zamkni vchodové dvere');

    expect(find.text('Čaká na potvrdenie'), findsOneWidget);
    expect(find.text('Vykonané'), findsNothing);
    expect(
      find.textContaining('nič sa nevykonalo a zariadenie sa nepohlo'),
      findsOneWidget,
    );
    expect(find.textContaining('Potvrdenie platí do'), findsOneWidget);
    // Čakanie na potvrdenie je samo rozhodnutie a audit ho dokladá, aj keď sa
    // nič nevykonalo.
    expect(
      find.textContaining('Čakalo na potvrdenie · confirmation_required'),
      findsOneWidget,
    );

    // Potvrdenie ide v tele, nie v adrese — je to jednorazové oprávnenie.
    final confirm = find.widgetWithText(FilledButton, 'Potvrdiť akciu');
    await tester.ensureVisible(confirm);
    await _settle(tester);
    await tester.tap(confirm);
    await _settle(tester);

    expect(find.text('Vykonané'), findsWidgets);
    final confirmations =
        seen.where((r) => r.url.path.endsWith('/v1/voice/confirmations'));
    expect(confirmations, hasLength(1));
    final body = jsonDecode((confirmations.single as http.Request).body)
        as Map<String, dynamic>;
    expect(body['confirmation_id'], '7f3ad21c');
    for (final request in seen) {
      expect(request.url.toString(), isNot(contains('7f3ad21c')));
    }
  });

  /// Zamietnutie nie je nepochopenie. Vypršané potvrdenie je vlastný dôvod a
  /// panel ho pomenuje.
  testWidgets('a refusal is shown with its reason, not as a failure to parse',
      (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(
        seen: seen,
        spoken: _awaiting(),
        spokenStatus: 202,
        confirmed: _refused('expired_confirmation'),
        confirmedStatus: 422,
      ),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _ownerToken);
    await _say(tester, 'zamkni vchodové dvere');

    final confirm = find.widgetWithText(FilledButton, 'Potvrdiť akciu');
    await tester.ensureVisible(confirm);
    await _settle(tester);
    await tester.tap(confirm);
    await _settle(tester);

    expect(find.text('Zamietnuté'), findsOneWidget);
    expect(find.text('Dôvod: expired_confirmation'), findsOneWidget);
    expect(find.text('Vykonané'), findsNothing);
    expect(find.text('Nevykonané'), findsNothing);
  });

  /// Hlas nedáva viac práv než panel. Hosťovi sa tlačidlo neukáže, pretože by
  /// mu jednotka odpovedala 403.
  testWidgets('a guest is told voice grants no more than the panel',
      (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(seen: seen, role: 'guest'),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _guestToken);

    expect(
      find.textContaining('Hlas nedáva viac práv než panel'),
      findsOneWidget,
    );
    expect(find.widgetWithText(TextField, 'Povel'), findsNothing);
    expect(find.widgetWithText(FilledButton, 'Odoslať povel'), findsNothing);
    expect(seen.where((r) => r.url.path.contains('/v1/voice/commands')),
        isEmpty);
  });

  /// Audit dokladá rozhodnutie, nie obsah. Prepis v ňom nie je ani vtedy, keď
  /// bol uložený — a panel ho tam nedoplní.
  testWidgets('the audit shows decisions and never the transcript',
      (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(
        seen: seen,
        audit: [
          _auditEvent('executed', 'device_confirmed'),
          _auditEvent('refused', 'role_not_permitted'),
        ],
      ),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _ownerToken);

    expect(find.text('Audit hlasu'), findsOneWidget);
    expect(find.textContaining('Vykonané · device_confirmed · pilot-owner'),
        findsOneWidget);
    expect(find.textContaining('Zamietnuté · role_not_permitted · pilot-owner'),
        findsOneWidget);
    expect(find.textContaining('audit dokladá rozhodnutie, nie obsah'),
        findsOneWidget);
  });
}
