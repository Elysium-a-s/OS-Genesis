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
