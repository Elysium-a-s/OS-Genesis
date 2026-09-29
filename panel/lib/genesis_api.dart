import 'dart:convert';

import 'package:http/http.dart' as http;

class GenesisDevice {
  const GenesisDevice({
    required this.id,
    required this.name,
    required this.power,
    required this.availability,
    required this.observedAt,
    required this.writable,
  });

  final String id;
  final String name;
  final bool? power;
  final String availability;
  final DateTime? observedAt;
  final bool writable;

  factory GenesisDevice.fromJson(Map<String, dynamic> json) => GenesisDevice(
        id: json['device_id'] as String,
        name: json['name'] as String,
        power: json['power'] as bool?,
        availability: json['availability'] as String,
        observedAt: DateTime.tryParse(json['observed_at'] as String? ?? ''),
        writable: json['writable'] as bool? ?? false,
      );

  bool isStale(DateTime now) =>
      availability != 'online' ||
      power == null ||
      observedAt == null ||
      now.difference(observedAt!) > const Duration(seconds: 60);
}

class GenesisCommandResult {
  const GenesisCommandResult(this.commandId, this.status, this.reason);
  final String commandId;
  final String status;
  final String? reason;

  factory GenesisCommandResult.fromJson(Map<String, dynamic> json) =>
      GenesisCommandResult(
        (json['request'] as Map<String, dynamic>)['command_id'] as String,
        json['status'] as String,
        json['reason'] as String?,
      );
}

class GenesisApi {
  GenesisApi({required this.baseUrl, http.Client? client})
      : _client = client ?? http.Client(),
        _ownsClient = client == null;

  final Uri baseUrl;
  final http.Client _client;
  final bool _ownsClient;

  Uri _path(String path) => baseUrl.resolve(path);

  Future<List<GenesisDevice>> devices(String readToken) async {
    final response = await _client.get(
      _path('/v1/devices'),
      headers: {'Authorization': 'Bearer $readToken'},
    ).timeout(const Duration(seconds: 5));
    if (response.statusCode != 200) {
      throw StateError('Genesis inventory HTTP ${response.statusCode}');
    }
    return (jsonDecode(response.body) as List<dynamic>)
        .map((item) => GenesisDevice.fromJson(item as Map<String, dynamic>))
        .toList();
  }

  Future<GenesisCommandResult> setPower({
    required String writeToken,
    required String householdId,
    required String deviceId,
    required bool value,
  }) async {
    final key = 'panel-${DateTime.now().microsecondsSinceEpoch}';
    final response = await _client.post(
      _path('/v1/commands'),
      headers: {
        'Authorization': 'Bearer $writeToken',
        'Content-Type': 'application/json',
      },
      body: jsonEncode({
        'household_id': householdId,
        'device_id': deviceId,
        'value': value,
        'idempotency_key': key,
        'correlation_id': key,
      }),
    ).timeout(const Duration(seconds: 12));
    if (response.statusCode != 200) {
      throw StateError('Genesis command HTTP ${response.statusCode}');
    }
    return GenesisCommandResult.fromJson(
      jsonDecode(response.body) as Map<String, dynamic>,
    );
  }

  void close() {
    if (_ownsClient) _client.close();
  }
}
