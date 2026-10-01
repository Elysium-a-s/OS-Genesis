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
const _memberToken = 'member-token-of-at-least-32-chars';
const _decisionId = 'ff77bdb0-70af-4f2a-a913-76609b66761b';

String _principal(String role) => jsonEncode({
      'household_id': 'pilot-home',
      'actor_id': role == 'owner' ? 'pilot-owner' : 'ivan',
      'role': role,
      'can_control_devices': true,
      'credential_id': role == 'owner' ? null : 'cred-1',
    });

/// Inventár s jedným svetlom, aby grant mohol mať názov zariadenia a nie
/// identifikátor. Diakritika je tu úmyselne — ide tou istou cestou ako inde.
String _inventory() => jsonEncode({
      'household': {'household_id': 'pilot-home', 'name': 'U Kováčov'},
      'areas': const [],
      'devices': [
        {
          'device_id': 'ha:light.living',
          'name': 'Obývačka — strop',
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
        'version': '0.1.0',
        'household_id': 'pilot-home',
        'home_assistant_configured': true,
      },
      'home_assistant': {
        'state': 'connected',
        'since': '2026-10-01T09:00:00Z',
        'last_inventory_at': '2026-10-01T09:05:00Z',
        'last_inventory_devices': 1,
        'last_error': null,
      },
    });

/// Jeden grant tak, ako ho vracia `GET /v1/access`.
String _grant({
  required String state,
  required String expiresAt,
  bool unlockConfirmed = true,
  int closeAttempts = 0,
  Map<String, dynamic>? lastConfirmed,
  List<Map<String, dynamic>> openIncidents = const [],
}) =>
    jsonEncode([
      {
        'grant': {
          'decision_id': _decisionId,
          'household_id': 'pilot-home',
          'device_id': 'ha:light.living',
          'capability_id': 'power',
          'granted_value': true,
          'expires_at': expiresAt,
          'state': state,
          'unlock_confirmed': unlockConfirmed,
          'unlock_command_id': 'behavior:ff77bdb0',
          'relock_command_id': null,
          'updated_at': '2026-10-01T09:00:00.000Z',
        },
        'required_confirmation': 'device',
        'close_attempts': closeAttempts,
        'last_confirmed': lastConfirmed,
        'open_incidents': openIncidents,
      }
    ]);

Map<String, dynamic> get _relockUncertain => {
      'incident_id': 'inc-1',
      'decision_id': _decisionId,
      'kind': 'relock_uncertain',
      'detail': 'the relock ended as unknown without the confirmation the '
          'decision required',
      'at': '2026-10-01T09:02:00.000Z',
      'resolved_at': null,
    };

Future<void> _settle(WidgetTester tester) async {
  for (var round = 0; round < 8; round++) {
    await tester.pump(const Duration(milliseconds: 20));
  }
}

/// Jednotka, ktorá o jednom grante hovorí to, čo si test vyžiada.
MockClient _unit({
  required List<http.BaseRequest> seen,
  required String grants,
  String? reconcileOutcome,
  String? grantsAfterReconcile,
  int grantsStatus = 200,
}) =>
    MockClient((request) async {
      seen.add(request);
      final path = request.url.path;
      final authorization = request.headers['Authorization'] ?? '';
      if (path.endsWith('/health')) return _ok(jsonEncode({'status': 'ok'}));
      if (path.contains('/reconcile')) {
        if (!authorization.contains(_ownerToken)) return _ok('{}', 403);
        final body = jsonDecode(grantsAfterReconcile ?? grants) as List<dynamic>;
        return _ok(jsonEncode({
          'outcome': reconcileOutcome ?? 'settled',
          'access': body.single,
        }));
      }
      if (path.endsWith('/v1/me')) {
        return _ok(_principal(
            authorization.contains(_ownerToken) ? 'owner' : 'member'));
      }
      if (path.endsWith('/v1/inventory')) return _ok(_inventory());
      if (path.endsWith('/v1/voice/audit')) return _ok('[]', 200);
      if (path.endsWith('/v1/access')) return _ok(grants, grantsStatus);
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

Future<void> _press(WidgetTester tester, Finder target) async {
  await tester.ensureVisible(target);
  await _settle(tester);
  await tester.tap(target);
  await _settle(tester);
}

String _inThePast() => DateTime.now()
    .toUtc()
    .subtract(const Duration(hours: 1))
    .toIso8601String();

String _inTheFuture() =>
    DateTime.now().toUtc().add(const Duration(hours: 2)).toIso8601String();

void main() {
  /// Toto je celý dôvod, prečo sekcia existuje: grant môže byť evidovaný ako
  /// vrátený a nikto nemusel potvrdiť, že sa zariadenie naozaj pohlo. Panel
  /// nesmie z logického stavu vyrobiť tvrdenie o fyzickom svete.
  testWidgets('an uncertain relock separates the ledger from the device',
      (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(
        seen: seen,
        grants: _grant(
          state: 'relock_pending',
          expiresAt: _inThePast(),
          closeAttempts: 2,
          openIncidents: [_relockUncertain],
        ),
      ),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _ownerToken);

    expect(find.text('ČASOVÝ PRÍSTUP'), findsOneWidget);
    expect(find.text('Neistý návrat'), findsOneWidget);
    // Názov z inventára, nie identifikátor. Zariadenie je aj v sekcii
    // Zariadenia, takže ten istý názov je na stránke dvakrát — podstatné je, že
    // grant neukazuje `ha:light.living`.
    expect(find.text('Obývačka — strop'), findsNWidgets(2));
    expect(find.text('ha:light.living'), findsNothing);

    // Dva samostatné riadky, nie jedna veta.
    expect(find.text('Evidencia jednotky'), findsOneWidget);
    expect(find.text('Fyzické potvrdenie'), findsOneWidget);
    expect(
      find.text('Žiadne potvrdenie. Fyzický stav zariadenia nie je známy.'),
      findsOneWidget,
    );
    expect(
      find.textContaining('Otvorený incident (relock_uncertain)'),
      findsOneWidget,
    );
    expect(find.text('Pokusy o uzavretie: 2'), findsOneWidget);
  });

  /// Potvrdenie poskytovateľom nie je potvrdenie zariadením. Keď rozhodnutie
  /// vyžadovalo druhé, panel to musí povedať, nie to schovať za „potvrdené".
  testWidgets('a provider acknowledgement is not passed off as the device',
      (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(
        seen: seen,
        grants: _grant(
          state: 'relocked',
          expiresAt: _inThePast(),
          lastConfirmed: {
            'command_id': 'relock-ff77bdb0',
            'value': false,
            'confirmation': 'provider',
            'at': '2026-10-01T09:30:00.000Z',
            'observed_at': null,
          },
        ),
      ),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _ownerToken);

    expect(
      find.textContaining('Potvrdil poskytovateľ (nie zariadenie):'),
      findsOneWidget,
    );
    expect(
      find.textContaining('Rozhodnutie vyžadovalo potvrdenie zariadením.'),
      findsOneWidget,
    );
    expect(find.textContaining('Zariadenie potvrdilo'), findsNothing);
  });

  /// Tlačidlo nesmie skracovať okno, ktoré ešte platí. Keby šlo, nebolo by to
  /// zosúladenie, ale odobranie prístupu — a ten, kto si ho zaslúžil, by o neho
  /// prišiel jedným omylom.
  testWidgets('a window that still holds offers no reconciliation',
      (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(
        seen: seen,
        grants: _grant(state: 'active', expiresAt: _inTheFuture()),
      ),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _ownerToken);

    expect(find.text('Otvorený'), findsOneWidget);
    final button = tester.widget<OutlinedButton>(
      find.widgetWithText(OutlinedButton, 'Zosúladiť teraz'),
    );
    expect(button.onPressed, isNull);
    expect(
      find.textContaining('Zatvoriť prístup skôr nie je zosúladenie'),
      findsOneWidget,
    );
    // A nič sa nevyžiadalo.
    expect(seen.where((r) => r.url.path.contains('reconcile')), isEmpty);
  });

  /// Expirovaný grant sa zosúladiť dá a výsledok sa prekreslí z odpovede.
  /// `attempted` sa nesmie zobraziť ako „zatvorené".
  testWidgets('an attempted close does not claim the access is shut',
      (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(
        seen: seen,
        grants: _grant(
          state: 'active',
          expiresAt: _inThePast(),
        ),
        reconcileOutcome: 'attempted',
        grantsAfterReconcile: _grant(
          state: 'relock_pending',
          expiresAt: _inThePast(),
          closeAttempts: 1,
          openIncidents: [_relockUncertain],
        ),
      ),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _ownerToken);

    expect(find.text('Po expirácii'), findsOneWidget);
    await _press(
      tester,
      find.widgetWithText(OutlinedButton, 'Zosúladiť teraz'),
    );

    expect(
      find.text('Uzavretie je zapísané, ale zariadenie ho nepotvrdilo. '
          'Incident zostáva otvorený.'),
      findsOneWidget,
    );
    expect(find.text('Neistý návrat'), findsOneWidget);
    expect(find.text('Vrátený'), findsNothing);

    // Identifikátor rozhodnutia nie je tajomstvo, ale token je — a ten v adrese
    // nie je ani tu.
    final requested =
        seen.where((r) => r.url.path.contains('reconcile')).toList();
    expect(requested, hasLength(1));
    expect(requested.single.method, 'POST');
    expect(requested.single.url.path, endsWith('/$_decisionId/reconcile'));
    for (final request in seen) {
      expect(request.url.toString(), isNot(contains(_ownerToken)));
    }
  });

  /// Zosúladenie je zásah do fyzického sveta domácnosti. Člen ho nevyžiada a
  /// panel mu neukáže tlačidlo, ktoré by mu jednotka odmietla.
  testWidgets('a member is told reconciliation is the owners', (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(
        seen: seen,
        grants: _grant(state: 'relock_pending', expiresAt: _inThePast()),
      ),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _memberToken);

    // Grant vidí — kto v domácnosti žije, má vedieť, že sa mu niečo zamyká samo.
    expect(find.text('Neistý návrat'), findsOneWidget);
    expect(find.text('Zosúladenie môže vyžiadať iba vlastník.'), findsOneWidget);
    expect(find.widgetWithText(OutlinedButton, 'Zosúladiť teraz'), findsNothing);
    expect(seen.where((r) => r.url.path.contains('reconcile')), isEmpty);
  });

  /// Prázdny zoznam a neprečítaný zoznam nie sú to isté a tu je ten rozdiel
  /// najdrahší: „žiadny prístup nie je otvorený" by bola nepravda o tom, čo je
  /// práve odomknuté.
  testWidgets('an unreadable overview never reads as nothing open',
      (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(seen: seen, grants: '{}', grantsStatus: 500),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _ownerToken);

    expect(
      find.text('Časové prístupy sa nepodarilo prečítať. Nevieme, či je '
          'niektorý otvorený.'),
      findsOneWidget,
    );
    expect(
      find.text('Žiadny časový prístup nie je otvorený ani nedovrený.'),
      findsNothing,
    );
  });

  /// Unlock bez zapísaného výsledku — to, čo zostane po reštarte jednotky medzi
  /// prijatím povelu a jeho výsledkom. Nesmie sa zobraziť ako otvorený prístup.
  testWidgets('a grant left without a result is not shown as open',
      (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(
        seen: seen,
        grants: _grant(
          state: 'granted',
          expiresAt: _inTheFuture(),
          unlockConfirmed: false,
        ),
      ),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _ownerToken);

    expect(find.text('Bez výsledku'), findsOneWidget);
    expect(find.text('Otvorený'), findsNothing);
    expect(
      find.textContaining('Unlock je prijatý, výsledok sa nezapísal'),
      findsOneWidget,
    );
    // Takýto grant prechod preberá bez ohľadu na expiráciu, takže tlačidlo
    // tlačiteľné je.
    final button = tester.widget<OutlinedButton>(
      find.widgetWithText(OutlinedButton, 'Zosúladiť teraz'),
    );
    expect(button.onPressed, isNotNull);
  });
}
