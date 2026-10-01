import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:genesis_panel/main.dart';
import 'package:genesis_panel/token_store.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

http.Response _ok(String body, [int status = 200]) =>
    http.Response.bytes(utf8.encode(body), status);

const _ownerToken = 'owner-token-of-at-least-32-characters';
const _memberToken = 'member-token-of-at-least-32-chars';

String _principal(String role) => jsonEncode({
      'household_id': 'pilot-home',
      'actor_id': role == 'owner' ? 'pilot-owner' : 'ivan',
      'role': role,
      'can_control_devices': true,
      'credential_id': role == 'owner' ? null : 'cred-1',
    });

String _inventory({bool roomsIncomplete = false, String link = 'connected'}) =>
    jsonEncode({
      'household': {'household_id': 'pilot-home', 'name': 'U Kováčov'},
      'areas': [
        {'area_id': 'ha:living', 'name': 'Obývačka', 'device_ids': const []}
      ],
      'devices': [
        {
          'device_id': 'ha:light.living',
          'name': 'Obývačka — strop',
          'power': false,
          'availability': 'online',
          'observed_at': '2026-10-01T09:00:00Z',
          'writable': true,
        }
      ],
      'home_assistant': {'state': link, 'rooms_incomplete': roomsIncomplete},
    });

String _diagnostics({String link = 'connected'}) => jsonEncode({
      'unit': {
        'version': '0.1.4',
        'household_id': 'pilot-home',
        'home_assistant_configured': true,
      },
      'home_assistant': {
        'state': link,
        'since': '2026-10-01T09:00:00Z',
        'last_inventory_at': '2026-10-01T09:05:00Z',
        'last_inventory_devices': 1,
        'last_error': link == 'connected' ? null : 'authentication',
      },
    });

/// Grant s otvoreným incidentom — to, čo robí prevádzkový stav zaujímavým.
String _grantWithIncident() => jsonEncode([
      {
        'grant': {
          'decision_id': 'ff77bdb0-70af-4f2a-a913-76609b66761b',
          'household_id': 'pilot-home',
          'device_id': 'ha:light.living',
          'capability_id': 'power',
          'granted_value': true,
          'expires_at': '2026-10-01T08:00:00.000Z',
          'state': 'relock_pending',
          'unlock_confirmed': true,
          'unlock_command_id': 'behavior:ff77bdb0',
          'relock_command_id': 'relock-ff77bdb0',
          'updated_at': '2026-10-01T08:05:00.000Z',
        },
        'required_confirmation': 'device',
        'close_attempts': 1,
        'last_confirmed': null,
        'open_incidents': [
          {
            'incident_id': 'inc-1',
            'decision_id': 'ff77bdb0-70af-4f2a-a913-76609b66761b',
            'kind': 'relock_uncertain',
            'detail': 'the relock ended as unknown',
            'at': '2026-10-01T08:05:00.000Z',
            'resolved_at': null,
          }
        ],
      }
    ]);

String _backup(String at, int bytes) => jsonEncode({
      'path': '/data/backups/genesis-ledger-${at.replaceAll(':', '-')}.sqlite3',
      'bytes': bytes,
      'at': at,
    });

Future<void> _settle(WidgetTester tester) async {
  for (var round = 0; round < 8; round++) {
    await tester.pump(const Duration(milliseconds: 20));
  }
}

Future<void> _press(WidgetTester tester, Finder target) async {
  await tester.ensureVisible(target);
  await _settle(tester);
  await tester.tap(target);
  await _settle(tester);
}

MockClient _unit({
  required List<http.BaseRequest> seen,
  String role = 'owner',
  String link = 'connected',
  bool roomsIncomplete = false,
  String grants = '[]',
  List<String>? backups,
  int backupListStatus = 200,
  int createStatus = 201,
}) {
  // Rastúci zoznam: mock doň po vytvorení zálohy pridáva, takže `const []` by
  // pri zápise padlo.
  final held = [...?backups];
  return MockClient((request) async {
    seen.add(request);
    final path = request.url.path;
    if (path.endsWith('/health')) return _ok(jsonEncode({'status': 'ok'}));
    if (path.endsWith('/v1/backup')) {
      if (request.method == 'POST') {
        if (createStatus != 201) return _ok('{}', createStatus);
        final made = _backup('2026-10-01T10:00:00.000Z', 40960);
        held.insert(0, made);
        return _ok(made, 201);
      }
      if (backupListStatus != 200) return _ok('{}', backupListStatus);
      return _ok('[${held.join(",")}]');
    }
    if (path.endsWith('/v1/me')) return _ok(_principal(role));
    if (path.endsWith('/v1/inventory')) {
      return _ok(_inventory(roomsIncomplete: roomsIncomplete, link: link));
    }
    if (path.endsWith('/v1/access')) return _ok(grants);
    if (path.endsWith('/v1/voice/audit')) return _ok('[]');
    if (path.endsWith('/v1/diagnostics')) return _ok(_diagnostics(link: link));
    if (path.endsWith('/v1/credentials')) return _ok('[]');
    if (path.endsWith('/v1/commands')) return _ok('[]');
    return _ok('{}', 404);
  });
}

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

void main() {
  /// Päť vecí, päť riadkov. Zliať ich do jedného „stav systému" by zahodilo
  /// presne tú informáciu, pre ktorú tu prevádzkovateľ je.
  testWidgets('five things that fail independently get five rows',
      (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(
        seen: seen,
        grants: _grantWithIncident(),
        backups: [_backup('2026-10-01T07:00:00.000Z', 20480)],
      ),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _ownerToken);

    expect(find.text('PREVÁDZKA'), findsOneWidget);
    expect(find.text('Genesis jednotka'), findsOneWidget);
    expect(find.text('Home Assistant'), findsOneWidget);
    expect(find.text('Inventár'), findsOneWidget);
    expect(find.text('Otvorené incidenty'), findsOneWidget);
    expect(find.text('Posledná záloha'), findsOneWidget);

    // Jednotka odpovedá a je to vlastný riadok, nie súčet so zvyškom.
    expect(find.text('Odpovedá'), findsOneWidget);
    // Incident sa počíta z grantov, ktoré panel už má.
    expect(find.text('1'), findsWidgets);
    expect(find.text('20 kB'), findsNothing);
    expect(find.textContaining('20 kB'), findsWidgets);
  });

  /// Zdravá jednotka nehovorí nič o Home Assistantovi. Toto je ten prípad, kde by
  /// jeden zlúčený stav klamal.
  testWidgets('a healthy unit does not vouch for Home Assistant',
      (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(seen: seen, link: 'disconnected'),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _ownerToken);

    expect(find.text('Odpovedá'), findsOneWidget);
    expect(find.text('Spojenie spadlo'), findsOneWidget);
    // Inventár je pri spadnutom spojení zastaraný, nie načítaný.
    expect(find.text('Zastaraný'), findsOneWidget);
  });

  /// Žiadna záloha je stav, ktorý treba povedať nahlas — nie prázdne miesto.
  testWidgets('no backup is stated plainly, not left blank', (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(seen: seen),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _ownerToken);

    expect(find.text('Žiadna'), findsOneWidget);
    expect(find.text('Žiadna záloha neexistuje'), findsOneWidget);

    await _press(tester, find.widgetWithText(FilledButton, 'Vytvoriť zálohu'));

    expect(find.text('Existuje'), findsOneWidget);
    expect(find.textContaining('Záloha je zapísaná'), findsOneWidget);
    final created = seen
        .where((r) => r.method == 'POST' && r.url.path.endsWith('/v1/backup'));
    expect(created, hasLength(1));
    // Token nie je v adrese ani tu.
    for (final request in seen) {
      expect(request.url.toString(), isNot(contains(_ownerToken)));
    }
  });

  /// Nenastavené zálohovanie nie je porucha. Zliať to s chybou by poslalo
  /// prevádzkovateľa hľadať problém, ktorý neexistuje.
  testWidgets('backups not configured is its own state, not a failure',
      (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(seen: seen, backupListStatus: 503),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _ownerToken);

    expect(find.text('Nenastavené'), findsOneWidget);
    expect(
      find.text('Jednotka nemá nastavený priečinok pre zálohy'),
      findsOneWidget,
    );
    // Tlačidlo, ktoré nemá kam zapisovať, sa neponúka.
    expect(find.widgetWithText(FilledButton, 'Vytvoriť zálohu'), findsNothing);
    expect(
      find.textContaining('Zálohy sa nepodarilo prečítať'),
      findsNothing,
    );
  });

  /// Jednotka existujúcu zálohu neprepíše a panel to povie ako fakt o
  /// bezpečnosti, nie ako zlyhanie.
  testWidgets('a colliding backup says the previous one is intact',
      (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(
        seen: seen,
        backups: [_backup('2026-10-01T07:00:00.000Z', 20480)],
        createStatus: 409,
      ),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _ownerToken);
    await _press(tester, find.widgetWithText(FilledButton, 'Vytvoriť zálohu'));

    expect(
      find.textContaining('Predchádzajúca záloha zostáva neporušená'),
      findsOneWidget,
    );
  });

  /// Zálohy patria vlastníkovi. Člen tlačidlo nevidí a panel sa o zoznam ani
  /// nepokúsi — jednotka by odpovedala 403.
  testWidgets('backups belong to the owner and the panel does not even ask',
      (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(seen: seen, role: 'member'),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _memberToken);

    expect(find.text('Len vlastník'), findsOneWidget);
    expect(find.text('Zálohy vidí iba vlastník domácnosti'), findsOneWidget);
    expect(find.widgetWithText(FilledButton, 'Vytvoriť zálohu'), findsNothing);
    expect(
      find.textContaining('Zálohu vytvára a vidí iba vlastník'),
      findsOneWidget,
    );
    expect(seen.where((r) => r.url.path.endsWith('/v1/backup')), isEmpty);
  });

  /// Obnovu panel nerobí a hovorí prečo. Tlačidlo „obnoviť" by mohlo poškodiť
  /// práve ten stav, ktorý má zachraňovať.
  testWidgets('the panel refuses to restore and explains why', (tester) async {
    final seen = <http.BaseRequest>[];
    await tester.pumpWidget(GenesisApp(
      client: _unit(seen: seen),
      tokenStore: InMemoryTokenStore(),
    ));
    await _signIn(tester, _ownerToken);

    expect(find.text('Obnova a rollback'), findsOneWidget);
    expect(
      find.textContaining('Obnovu panel nerobí a nebude'),
      findsOneWidget,
    );
    expect(
      find.textContaining('docs/ELYSIUM-348-obnova.md'),
      findsOneWidget,
    );
    // Žiadne tlačidlo, ktoré by obnovu predstieralo.
    expect(find.widgetWithText(FilledButton, 'Obnoviť'), findsNothing);
    expect(find.widgetWithText(OutlinedButton, 'Obnoviť'), findsNothing);
  });
}
