import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:genesis_panel/genesis_api.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

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
        return http.Response(jsonEncode({
          'household_id': 'pilot-home',
          'actor_id': 'pilot-member',
          'role': 'member',
          'can_control_devices': true,
        }), 200);
      }
      if (request.url.path == '/v1/devices') {
        return http.Response(jsonEncode([
          {
            'device_id': 'ha:light.living',
            'name': 'Living',
            'power': false,
            'availability': 'online',
            'observed_at': '2026-09-29T12:00:00Z',
            'writable': true
          }
        ]), 200);
      }
      return http.Response(jsonEncode({
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
    final devices = await api.devices('read-secret');
    expect(devices.single.id, 'ha:light.living');
    final result = await api.setPower(
      writeToken: 'write-secret',
      householdId: 'pilot-home',
      deviceId: devices.single.id,
      value: true,
    );
    expect(result.status, 'provider_confirmed');
    expect(calls[0].headers['Authorization'], 'Bearer member-secret');
    expect(calls[1].headers['Authorization'], 'Bearer read-secret');
    expect(calls[2].headers['Authorization'], 'Bearer write-secret');
    expect(jsonDecode(calls[2].body)['value'], true);
  });

  test('a command snapshot carries its evidence, time and reference', () async {
    final client = MockClient((request) async => http.Response(
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
      return http.Response(
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

  test('API paths stay under the Home Assistant ingress prefix', () async {
    final paths = <String>[];
    final client = MockClient((request) async {
      paths.add(request.url.path);
      if (request.url.path.endsWith('/v1/me')) {
        return http.Response(jsonEncode({
          'household_id': 'pilot-home',
          'actor_id': 'pilot-owner',
          'role': 'owner',
          'can_control_devices': true,
        }), 200);
      }
      return http.Response('[]', 200);
    });
    final api = GenesisApi(
      baseUrl: Uri.parse('https://home.example/api/hassio_ingress/pilot/'),
      client: client,
    );
    await api.me('owner-secret');
    await api.devices('owner-secret');
    expect(paths, [
      '/api/hassio_ingress/pilot/v1/me',
      '/api/hassio_ingress/pilot/v1/devices',
    ]);
  });
}
