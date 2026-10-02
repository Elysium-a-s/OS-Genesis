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
const _pairingCode = 'PAIR-7Q2M-4KX9';

String _principal(String role, String actor) => jsonEncode({
      'household_id': 'pilot-home',
      'actor_id': actor,
      'role': role,
      'can_control_devices': role != 'guest',
      'credential_id': role == 'owner' ? null : 'cred-1',
    });

String _inventory() => jsonEncode({
      'household': {'household_id': 'pilot-home', 'name': 'U Kováčov'},
      'areas': const [],
      'devices': const [],
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
        'last_inventory_devices': 0,
        'last_error': null,
      },
    });

String _credentials({bool revoked = false}) => jsonEncode([
      {
        'credential_id': 'cred-1',
        'household_id': 'pilot-home',
        'role': 'member',
        'actor_id': 'ivan',
        'issued_at': '2026-10-01T08:00:00Z',
        'last_used_at': revoked ? '2026-10-01T08:30:00Z' : null,
        'revoked_at': revoked ? '2026-10-01T09:10:00Z' : null,
        'revoked_by': revoked ? 'pilot-owner' : null,
      }
    ]);

Future<void> _settle(WidgetTester tester) async {
  for (var round = 0; round < 8; round++) {
    await tester.pump(const Duration(milliseconds: 20));
  }
}

/// Jednotka, ktorá rozlišuje rolu podľa predloženého tokenu.
///
/// `seen` zbiera každú požiadavku, takže test môže tvrdiť, kde tajomstvá
/// **neboli** — v adrese ani v tele tam, kde nepatria.
MockClient _unit({
  required List<http.BaseRequest> seen,
  bool codeStillWorks = true,
  bool tokenWasRevoked = false,
  List<String>? revoked,
}) =>
    MockClient((request) async {
      seen.add(request);
      final path = request.url.path;
      final authorization = request.headers['Authorization'] ?? '';
      if (path.endsWith('/health')) {
        return _ok(jsonEncode({'status': 'ok'}));
      }
      if (path.endsWith('/v1/pairings/redeem')) {
        if (!codeStillWorks) return _ok('{}', 422);
        return _ok(jsonEncode({
          'credential_id': 'cred-2',
          'household_id': 'pilot-home',
          'role': 'member',
          'actor_id': 'ivan',
          'token': _memberToken,
        }));
      }
      if (path.endsWith('/v1/pairings')) {
        if (!authorization.contains(_ownerToken)) return _ok('{}', 403);
        final body = jsonDecode(request.body) as Map<String, dynamic>;
        return _ok(
            jsonEncode({
              'pairing_id': 'pair-1',
              'role': body['role'],
              'actor_id': body['actor_id'],
              'expires_at': '2026-10-01T10:00:00Z',
              'code': _pairingCode,
            }),
            201);
      }
      if (path.contains('/v1/credentials/')) {
        if (!authorization.contains(_ownerToken)) return _ok('{}', 403);
        revoked?.add(path.split('/').last);
        return _ok('', 204);
      }
      if (path.endsWith('/v1/credentials')) {
        if (!authorization.contains(_ownerToken)) return _ok('{}', 403);
        return _ok(_credentials(revoked: (revoked ?? const []).isNotEmpty));
      }
      // Všetko ostatné je čítanie domácnosti a potrebuje platný token.
      if (tokenWasRevoked && authorization.contains(_memberToken)) {
        return _ok('{}', 401);
      }
      if (path.endsWith('/v1/me')) {
        return _ok(authorization.contains(_ownerToken)
            ? _principal('owner', 'pilot-owner')
            : _principal('member', 'ivan'));
      }
      if (path.endsWith('/v1/inventory')) return _ok(_inventory());
      if (path.endsWith('/v1/voice/audit')) return _ok('[]', 200);
      if (path.endsWith('/v1/access')) return _ok('[]', 200);
      if (path.endsWith('/v1/diagnostics')) return _ok(_diagnostics());
      if (path.endsWith('/v1/commands')) return _ok('[]');
      return _ok('{}', 404);
    });

/// Ťuknutie, ktoré prežije rast stránky.
///
/// `ensureVisible` scroll iba spustí; bez doznenia animácie ťuknutie minie cieľ,
/// a pridanie sekcie vyššie na stránke tak zlomí test, ktorý s ňou nesúvisí.
Future<void> _press(WidgetTester tester, Finder target) async {
  await tester.ensureVisible(target);
  await _settle(tester);
  await tester.tap(target);
  await _settle(tester);
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
  Future<void> pumpPanel(
    WidgetTester tester,
    MockClient client,
    GenesisTokenStore store,
  ) async {
    tester.view.physicalSize = const Size(1200, 2000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(GenesisApp(client: client, tokenStore: store));
    await _settle(tester);
    // Aj uplatnenie kódu potrebuje adresu jednotky: nainštalovaná aplikácia sa
    // otvorí bez nej a člen s kódom sa tiež musí dozvedieť, kde jednotka je.
    await tester.enterText(
      find.widgetWithText(TextField, 'Adresa Genesis API'),
      'http://green.local:8080',
    );
    // Uložený token sa obnovuje pri štarte, teda skôr než adresa existuje, a
    // vtedy sa nič nepokúša — rovnako ako na zariadení. Až vlastný pätnásťsekundový
    // cyklus panelu ho použije, a až tam sa ukáže, že ho jednotka odmieta.
    await tester.pump(const Duration(seconds: 15));
    await _settle(tester);
  }

  testWidgets('an owner issues a code, sees it once, and then it is gone',
      (tester) async {
    final seen = <http.BaseRequest>[];
    final store = InMemoryTokenStore();
    await pumpPanel(tester, _unit(seen: seen), store);
    await _signIn(tester, _ownerToken);

    // Vlastník má správu prístupu.
    expect(find.text('Vydať párovací kód'), findsOneWidget);
    expect(find.text('Vydané identity'), findsOneWidget);
    // A vidí, komu je čo vydané, aj či to už niekto použil.
    expect(find.text('ivan'), findsOneWidget);
    expect(find.text('Zatiaľ nepoužité'), findsOneWidget);

    await tester.enterText(
      find.widgetWithText(TextField, 'Identifikátor člena'),
      'zuzana',
    );
    await _press(tester, find.text('Vydať kód'));
    await _settle(tester);

    // Kód sa zobrazí, a panel povie, že druhýkrát nebude.
    expect(find.text(_pairingCode), findsOneWidget);
    expect(find.text('Párovací kód pre zuzana'), findsOneWidget);
    expect(
      find.textContaining('Zobrazuje sa raz', findRichText: true),
      findsOneWidget,
    );

    // Rola išla v tele, nie v adrese.
    final issue = seen.firstWhere((r) => r.url.path.endsWith('/v1/pairings'));
    expect(jsonDecode((issue as http.Request).body)['role'], 'member');
    expect(jsonDecode(issue.body)['actor_id'], 'zuzana');

    // Kód musí byť čitateľný aj čítačkou obrazovky. SelectableText svoj obsah do
    // stromu prístupnosti sám nedáva — na rozdiel od Text — a kód sa pritom
    // ukazuje práve raz. Bez popisu by si ho nevidiaci vlastník nemal ako
    // prečítať a člena do domácnosti by nepridal.
    final code = tester.widget<SelectableText>(
      find.widgetWithText(SelectableText, _pairingCode),
    );
    expect(code.semanticsLabel, isNotNull);
    expect(
      code.semanticsLabel!.replaceAll(' ', ''),
      _pairingCode,
      reason: 'popis pre čítačku musí niesť ten istý kód',
    );
    expect(
      code.semanticsLabel, contains(' '),
      reason: 'kód sa číta po skupinách, nie ako jeden zhluk znakov',
    );

    await _press(tester, find.text('Mám ho'));
    await _settle(tester);
    expect(find.text(_pairingCode), findsNothing);
  });

  testWidgets('a member cannot issue or revoke anything', (tester) async {
    final seen = <http.BaseRequest>[];
    await pumpPanel(tester, _unit(seen: seen), InMemoryTokenStore());
    await _signIn(tester, _memberToken);

    // Inventár áno, správa prístupu nie.
    expect(find.text('U Kováčov'), findsWidgets);
    expect(find.text('Vydať párovací kód'), findsNothing);
    expect(find.text('Vydať kód'), findsNothing);
    expect(find.text('Vydané identity'), findsNothing);
    expect(find.text('Odobrať'), findsNothing);
    expect(
      find.textContaining('môže iba vlastník domácnosti', findRichText: true),
      findsOneWidget,
    );
    // Panel sa o zoznam identít ani nepokúsil.
    expect(seen.any((r) => r.url.path.endsWith('/v1/credentials')), isFalse);

    // Uplatniť kód smie každý — to je jediná cesta, ako sa k prístupu dostať.
    expect(find.text('Mám párovací kód'), findsOneWidget);
  });

  testWidgets('a code that no longer works leaves the panel without a token',
      (tester) async {
    final seen = <http.BaseRequest>[];
    final store = InMemoryTokenStore();
    await pumpPanel(
      tester,
      _unit(seen: seen, codeStillWorks: false),
      store,
    );

    await tester.enterText(
      find.widgetWithText(TextField, 'Domácnosť'),
      'pilot-home',
    );
    await tester.enterText(
      find.widgetWithText(TextField, 'Párovací kód'),
      _pairingCode,
    );
    await _press(tester, find.text('Uplatniť kód'));
    await _settle(tester);

    // Vypršaný, už uplatnený a odobraný kód vyzerajú rovnako — panel nehádá,
    // ktorý z nich to bol.
    expect(
      find.textContaining('Mohol vypršať, už byť uplatnený alebo odobraný',
          findRichText: true),
      findsOneWidget,
    );
    expect(await store.read(), isNull);

    // Kód išiel v tele a nikdy v adrese.
    final redeem =
        seen.firstWhere((r) => r.url.path.endsWith('/v1/pairings/redeem'));
    expect(redeem.url.toString(), isNot(contains(_pairingCode)));
    expect(jsonDecode((redeem as http.Request).body)['code'], _pairingCode);
    for (final request in seen) {
      expect(request.url.toString(), isNot(contains(_pairingCode)));
    }
  });

  testWidgets('a redeemed code is stored and never shown again', (tester) async {
    final seen = <http.BaseRequest>[];
    final store = InMemoryTokenStore();
    await pumpPanel(tester, _unit(seen: seen), store);

    await tester.enterText(
      find.widgetWithText(TextField, 'Domácnosť'),
      'pilot-home',
    );
    await tester.enterText(
      find.widgetWithText(TextField, 'Párovací kód'),
      _pairingCode,
    );
    await _press(tester, find.text('Uplatniť kód'));
    await _settle(tester);

    // Token je v úložisku a panel ožil.
    expect(await store.read(), _memberToken);
    expect(find.text('U Kováčov'), findsWidgets);

    // Token nie je v žiadnej adrese — ide v hlavičke.
    for (final request in seen) {
      expect(request.url.toString(), isNot(contains(_memberToken)));
    }
    expect(
      seen.any((r) => (r.headers['Authorization'] ?? '').contains(_memberToken)),
      isTrue,
    );
    // A nikde sa nevykreslí ako čitateľný text. Jediné miesto, kde hodnota je,
    // je zakryté pole — `find.text` by tam EditableText našiel, takže sa tvrdí
    // to, čo naozaj platí: žiadny `Text` ho neukazuje a pole je obscured.
    final tokenField = tester.widget<TextField>(
      find.widgetWithText(TextField, 'Prístupový token'),
    );
    expect(tokenField.obscureText, isTrue);
    final rendered = tester
        .widgetList<Text>(find.byType(Text))
        .map((text) => text.data ?? '');
    expect(rendered.any((value) => value.contains(_memberToken)), isFalse);
  });

  testWidgets('revoking a credential takes the panel access away',
      (tester) async {
    final seen = <http.BaseRequest>[];
    final revoked = <String>[];
    final store = InMemoryTokenStore();
    await store.write(_memberToken);

    // Jednotka už tento token nepozná — presne to vidí panel po odobraní.
    await pumpPanel(
      tester,
      _unit(seen: seen, tokenWasRevoked: true, revoked: revoked),
      store,
    );
    await _settle(tester);

    // Token sa zahodil a panel hovorí jedinú vec, ktorá pomôže.
    expect(await store.read(), isNull);
    expect(
      find.textContaining('Požiadaj vlastníka o nový párovací kód',
          findRichText: true),
      findsOneWidget,
    );
    // Neskúša to znova dokola: po odmietnutí je pole prázdne, takže cyklus
    // panelu naň nesiahne.
    final before = seen.length;
    await tester.pump(const Duration(seconds: 15));
    await _settle(tester);
    final authorized =
        seen.skip(before).where((r) => r.headers.containsKey('Authorization'));
    expect(authorized, isEmpty);
  });

  testWidgets('an owner revokes a credential and the list says so',
      (tester) async {
    final seen = <http.BaseRequest>[];
    final revoked = <String>[];
    await pumpPanel(
      tester,
      _unit(seen: seen, revoked: revoked),
      InMemoryTokenStore(),
    );
    await _signIn(tester, _ownerToken);

    expect(find.text('Odobrať'), findsOneWidget);
    await _press(tester, find.text('Odobrať'));
    await _settle(tester);

    expect(revoked, ['cred-1']);
    expect(find.text('Odobrané'), findsOneWidget);
    expect(find.text('Odobrať'), findsNothing);
    // Identifikátor kreditívy nie je tajomstvo, takže v ceste byť môže; token
    // vlastníka v nej nie je.
    final call = seen.firstWhere((r) => r.url.path.contains('/v1/credentials/'));
    expect(call.method, 'DELETE');
    expect(call.url.toString(), contains('cred-1'));
    expect(call.url.toString(), isNot(contains(_ownerToken)));
  });
}
