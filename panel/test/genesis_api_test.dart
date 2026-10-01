import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:genesis_panel/genesis_api.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

/// Odpoveď tak, ako ju posiela jednotka: JSON v UTF-8 bez `charset` v hlavičke.
/// `http.Response` so stringom by telo kódoval Latin-1 a na prvom „č" by spadol,
/// a presne preto panel číta bajty a nie `response.body`.
http.Response _ok(String body, [int status = 200]) =>
    http.Response.bytes(utf8.encode(body), status);

void main() {
  test('availability distinguishes current and unknown HA state', () {
    final now = DateTime.utc(2026, 9, 29, 12);
    final current = GenesisDevice(
      id: 'ha:light.living',
      name: 'Living',
      power: true,
      availability: 'online',
      observedAt: now.subtract(const Duration(seconds: 5)),
      writable: true,
    );
    expect(current.isStale, false);
    final unchanged = GenesisDevice(
      id: current.id,
      name: current.name,
      power: true,
      availability: 'online',
      observedAt: now.subtract(const Duration(days: 1)),
      writable: true,
    );
    expect(unchanged.isStale, false);
    final unknown = GenesisDevice(
      id: current.id,
      name: current.name,
      power: null,
      availability: 'unknown',
      observedAt: current.observedAt,
      writable: true,
    );
    expect(unknown.isStale, true);
  });

  test('typed client reads devices and sends a power command', () async {
    final calls = <http.Request>[];
    final client = MockClient((request) async {
      calls.add(request);
      if (request.url.path == '/v1/me') {
        return _ok(jsonEncode({
          'household_id': 'pilot-home',
          'actor_id': 'pilot-member',
          'role': 'member',
          'can_control_devices': true,
        }), 200);
      }
      if (request.url.path == '/v1/inventory') {
        return _ok(jsonEncode({
          'household': {'household_id': 'pilot-home', 'name': 'Doma'},
          'areas': [
            {
              'area_id': 'ha:living_room',
              'name': 'Obývačka',
              'device_ids': ['ha:light.living']
            }
          ],
          'devices': [
            {
              'device_id': 'ha:light.living',
              'name': 'Living',
              'power': false,
              'availability': 'online',
              'observed_at': '2026-09-29T12:00:00Z',
              'writable': true,
              'area_id': 'ha:living_room',
              'area_name': 'Obývačka'
            }
          ],
          'home_assistant': {'state': 'connected', 'rooms_incomplete': false}
        }), 200);
      }
      return _ok(jsonEncode({
        'request': {'command_id': 'cmd-1'},
        'status': 'provider_confirmed',
        'reason': null
      }), 200);
    });
    final api = GenesisApi(
      baseUrl: Uri.parse('http://green.local:8765'),
      client: client,
    );
    final me = await api.me('member-secret');
    expect(me.role, 'member');
    expect(me.canControlDevices, true);
    final inventory = await api.inventory('read-secret');
    expect(inventory.householdName, 'Doma');
    expect(inventory.devices.single.id, 'ha:light.living');
    expect(inventory.devices.single.areaName, 'Obývačka');
    expect(inventory.areas.single.deviceIds, ['ha:light.living']);
    expect(inventory.isConnected, true);
    final result = await api.setPower(
      writeToken: 'write-secret',
      householdId: 'pilot-home',
      deviceId: inventory.devices.single.id,
      value: true,
    );
    expect(result.status, 'provider_confirmed');
    expect(calls[0].headers['Authorization'], 'Bearer member-secret');
    expect(calls[1].headers['Authorization'], 'Bearer read-secret');
    expect(calls[2].headers['Authorization'], 'Bearer write-secret');
    expect(jsonDecode(calls[2].body)['value'], true);
  });

  test('a command snapshot carries its evidence, time and reference', () async {
    final client = MockClient((request) async => _ok(
          jsonEncode({
            'request': {
              'command_id': 'cmd-1',
              'device_id': 'ha:light.living',
              'correlation_id': 'panel-42',
            },
            'status': 'device_confirmed',
            'confirmation_level': 'device',
            'status_changed_at': '2026-09-30T09:06:00Z',
            'reason': null,
            'evidence': {
              'kind': 'device_observation',
              'reference': 'light.living=on',
              'at': '2026-09-30T09:06:00Z',
            },
          }),
          200,
        ));
    final api = GenesisApi(
      baseUrl: Uri.parse('http://green.local:8765'),
      client: client,
    );
    final command = await api.command('read-secret', 'cmd-1');
    expect(command.status, 'device_confirmed');
    expect(command.correlationId, 'panel-42');
    expect(command.statusChangedAt, DateTime.utc(2026, 9, 30, 9, 6));
    expect(command.evidence!.kind, 'device_observation');
    expect(command.evidence!.reference, 'light.living=on');
    expect(command.isSettled, true);
    expect(command.isUncertain, false);
  });

  test('an uncertain command keeps what the provider confirmed and is not settled',
      () async {
    final command = GenesisCommand.fromJson({
      'request': {'command_id': 'cmd-2', 'correlation_id': 'panel-7'},
      'status': 'unknown',
      // Prechod na neistý stav dôkaz zo snapshotu zmaže, úroveň potvrdenia nie.
      'confirmation_level': 'provider',
      'status_changed_at': '2026-09-30T09:07:00Z',
      'reason': 'no_device_confirmation',
      'evidence': null,
    });
    expect(command.isUncertain, true);
    expect(command.isInFlight, false);
    expect(command.isSettled, false);
    expect(command.providerAcknowledged, true);
    expect(command.evidence, isNull);

    // Stav, ktorý panel nepozná, sa nesmie vyhodnotiť ako dokončený ani ako
    // prebiehajúci — inak by budúci siedmy stav prešiel ako úspech.
    final unfamiliar = GenesisCommand.fromJson({
      'request': {'command_id': 'cmd-3', 'correlation_id': 'panel-8'},
      'status': 'superseded',
      'confirmation_level': 'none',
      'status_changed_at': '2026-09-30T09:08:00Z',
      'reason': null,
      'evidence': null,
    });
    expect(unfamiliar.isSettled, false);
    expect(unfamiliar.isInFlight, false);
    expect(unfamiliar.isUncertain, false);
  });

  test('diagnostics separates the unit from the Home Assistant link', () async {
    final paths = <String>[];
    final client = MockClient((request) async {
      paths.add(request.url.path);
      return _ok(
          jsonEncode({
            'unit': {
              'version': '0.1.0',
              'household_id': 'pilot-home',
              'home_assistant_configured': true,
            },
            'home_assistant': {
              'state': 'disconnected',
              'since': '2026-09-30T09:00:00Z',
              'last_inventory_at': '2026-09-30T08:55:00Z',
              'last_inventory_devices': 3,
              'last_error': 'authentication',
            },
          }),
          200);
    });
    final api = GenesisApi(
      baseUrl: Uri.parse('http://green.local:8765'),
      client: client,
    );
    final diagnostics = await api.diagnostics('read-secret');
    expect(paths.single, '/v1/diagnostics');
    expect(diagnostics.homeAssistantConfigured, true);
    expect(diagnostics.homeAssistant.isConnected, false);
    expect(diagnostics.homeAssistant.isUnconfigured, false);
    expect(diagnostics.homeAssistant.lastInventoryDevices, 3);
    expect(diagnostics.homeAssistant.lastError, 'authentication');
  });

  test('an unconfigured link is a state of its own, not a failure', () {
    final link = GenesisHaLink.fromJson({
      'state': 'not_configured',
      'since': '2026-09-30T09:00:00Z',
      'last_inventory_at': null,
      'last_inventory_devices': 0,
      'last_error': null,
    });
    expect(link.isUnconfigured, true);
    expect(link.isConnected, false);
    expect(link.lastError, isNull);
  });

  test('a refused token has its own type, so it is not retried as a glitch', () async {
    final client = MockClient((request) async => _ok('{}', 401));
    final api = GenesisApi(
      baseUrl: Uri.parse('http://green.local:8765'),
      client: client,
    );
    // Odobraná kreditíva nie je chyba siete: panel ju musí rozlíšiť, inak by
    // token skúšal dokola.
    expect(() => api.me('revoked-token'), throwsA(isA<GenesisUnauthorized>()));
    expect(
      () => api.inventory('revoked-token'),
      throwsA(isA<GenesisUnauthorized>()),
    );
    expect(
      () => api.credentials('revoked-token'),
      throwsA(isA<GenesisUnauthorized>()),
    );
    expect(
      () => api.revokeCredential(
        ownerToken: 'revoked-token',
        credentialId: 'cred-1',
      ),
      throwsA(isA<GenesisUnauthorized>()),
    );
    // Guest, ktorý skúsi správu, dostane 403 — pre panel je to to isté: tento
    // token na to nemá.
    final forbidden = MockClient((request) async => _ok('{}', 403));
    final guest = GenesisApi(
      baseUrl: Uri.parse('http://green.local:8765'),
      client: forbidden,
    );
    expect(
      () => guest.credentials('guest-token'),
      throwsA(isA<GenesisUnauthorized>()),
    );
  });

  test('a pairing code and a token travel in the body, never in the path',
      () async {
    final seen = <http.Request>[];
    final client = MockClient((request) async {
      seen.add(request);
      if (request.url.path.endsWith('/v1/pairings/redeem')) {
        return _ok(jsonEncode({
          'credential_id': 'cred-2',
          'household_id': 'pilot-home',
          'role': 'member',
          'actor_id': 'ivan',
          'token': 'issued-token-value',
        }));
      }
      return _ok(
          jsonEncode({
            'pairing_id': 'pair-1',
            'role': 'guest',
            'actor_id': 'zuzana',
            'expires_at': '2026-10-01T10:00:00Z',
            'code': 'PAIR-CODE-VALUE',
          }),
          201);
    });
    final api = GenesisApi(
      baseUrl: Uri.parse('http://green.local:8765'),
      client: client,
    );
    final pairing = await api.createPairing(
      ownerToken: 'owner-token-value',
      householdId: 'pilot-home',
      role: 'guest',
      actorId: 'zuzana',
    );
    expect(pairing.code, 'PAIR-CODE-VALUE');
    expect(pairing.role, 'guest');
    expect(pairing.expiresAt, DateTime.utc(2026, 10, 1, 10));

    final issued = await api.redeemPairing(
      householdId: 'pilot-home',
      code: 'PAIR-CODE-VALUE',
    );
    expect(issued.token, 'issued-token-value');

    for (final request in seen) {
      final url = request.url.toString();
      expect(url, isNot(contains('PAIR-CODE-VALUE')));
      expect(url, isNot(contains('owner-token-value')));
      expect(url, isNot(contains('issued-token-value')));
    }
    // Vlastníkov token ide v hlavičke; kód uplatnenia žiadny token nepotrebuje,
    // pretože kód sám je oprávnenie.
    expect(seen.first.headers['Authorization'], 'Bearer owner-token-value');
    expect(seen.last.headers.containsKey('Authorization'), isFalse);
  });

  test('a credential view says whether it was used and whether it was revoked',
      () {
    final unused = GenesisCredential.fromJson({
      'credential_id': 'cred-1',
      'household_id': 'pilot-home',
      'role': 'member',
      'actor_id': 'ivan',
      'issued_at': '2026-10-01T08:00:00Z',
      'last_used_at': null,
      'revoked_at': null,
      'revoked_by': null,
    });
    expect(unused.isUnused, isTrue);
    expect(unused.isRevoked, isFalse);

    final revoked = GenesisCredential.fromJson({
      'credential_id': 'cred-1',
      'household_id': 'pilot-home',
      'role': 'member',
      'actor_id': 'ivan',
      'issued_at': '2026-10-01T08:00:00Z',
      'last_used_at': '2026-10-01T08:30:00Z',
      'revoked_at': '2026-10-01T09:10:00Z',
      'revoked_by': 'pilot-owner',
    });
    expect(revoked.isRevoked, isTrue);
    expect(revoked.isUnused, isFalse);
    expect(revoked.revokedBy, 'pilot-owner');
  });

  test('API paths stay under the Home Assistant ingress prefix', () async {
    final paths = <String>[];
    final client = MockClient((request) async {
      paths.add(request.url.path);
      if (request.url.path.endsWith('/v1/me')) {
        return _ok(jsonEncode({
          'household_id': 'pilot-home',
          'actor_id': 'pilot-owner',
          'role': 'owner',
          'can_control_devices': true,
        }), 200);
      }
      return _ok(
          jsonEncode({
            'household': {'household_id': 'pilot-home', 'name': null},
            'areas': [],
            'devices': [],
            'home_assistant': {'state': 'connecting', 'rooms_incomplete': false}
          }),
          200);
    });
    final api = GenesisApi(
      baseUrl: Uri.parse('https://home.example/api/hassio_ingress/pilot/'),
      client: client,
    );
    await api.me('owner-secret');
    await api.inventory('owner-secret');
    expect(paths, [
      '/api/hassio_ingress/pilot/v1/me',
      '/api/hassio_ingress/pilot/v1/inventory',
    ]);
  });

  test('an empty inventory says whether anyone could look', () {
    final unreachable = GenesisInventory.fromJson({
      'household': {'household_id': 'pilot-home', 'name': null},
      'areas': [],
      'devices': [],
      'home_assistant': {'state': 'disconnected', 'rooms_incomplete': false},
    });
    expect(unreachable.isConnected, false);
    expect(unreachable.devices, isEmpty);
    // Prázdno bez názvu domácnosti si panel nesmie domyslieť.
    expect(unreachable.householdName, isNull);

    final reallyEmpty = GenesisInventory.fromJson({
      'household': {'household_id': 'pilot-home', 'name': 'Doma'},
      'areas': [],
      'devices': [],
      'home_assistant': {'state': 'connected', 'rooms_incomplete': false},
    });
    expect(reallyEmpty.isConnected, true);
    expect(reallyEmpty.devices, isEmpty);
  });

  test('a device without a room is not put into an invented one', () {
    final inventory = GenesisInventory.fromJson({
      'household': {'household_id': 'pilot-home', 'name': 'Doma'},
      'areas': [
        {'area_id': 'ha:living_room', 'name': 'Obývačka', 'device_ids': []}
      ],
      'devices': [
        {
          'device_id': 'ha:switch.plug',
          'name': 'Zásuvka',
          'power': true,
          'availability': 'online',
          'writable': true,
          'area_id': null,
          'area_name': null
        }
      ],
      'home_assistant': {'state': 'connected', 'rooms_incomplete': true},
    });
    expect(inventory.devicesWithoutArea.single.id, 'ha:switch.plug');
    expect(inventory.areas.single.deviceIds, isEmpty);
    // Chýbajúce miestnosti sú priznané, nie vydávané za prázdnu domácnosť.
    expect(inventory.roomsIncomplete, true);
  });
}
