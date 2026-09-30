import 'dart:convert';

import 'package:http/http.dart' as http;

class GenesisPrincipal {
  const GenesisPrincipal({
    required this.householdId,
    required this.actorId,
    required this.role,
    required this.canControlDevices,
  });

  final String householdId;
  final String actorId;
  final String role;
  final bool canControlDevices;

  factory GenesisPrincipal.fromJson(Map<String, dynamic> json) =>
      GenesisPrincipal(
        householdId: json['household_id'] as String,
        actorId: json['actor_id'] as String,
        role: json['role'] as String,
        canControlDevices: json['can_control_devices'] as bool,
      );
}

class GenesisDevice {
  const GenesisDevice({
    required this.id,
    required this.name,
    required this.power,
    required this.availability,
    required this.observedAt,
    required this.writable,
    this.areaId,
    this.areaName,
  });

  final String id;
  final String name;
  final bool? power;
  final String availability;
  final DateTime? observedAt;
  final bool writable;

  /// Miestnosť podľa Home Assistanta. Chýba, keď zariadenie žiadnu nemá — je to
  /// legitímny stav domácnosti, nie chýbajúci údaj.
  final String? areaId;
  final String? areaName;

  factory GenesisDevice.fromJson(Map<String, dynamic> json) => GenesisDevice(
        id: json['device_id'] as String,
        name: json['name'] as String,
        power: json['power'] as bool?,
        availability: json['availability'] as String,
        observedAt: DateTime.tryParse(json['observed_at'] as String? ?? ''),
        writable: json['writable'] as bool? ?? false,
        areaId: json['area_id'] as String?,
        areaName: json['area_name'] as String?,
      );

  bool get isStale => availability != 'online' || power == null;
}

/// Miestnosť domácnosti tak, ako ju pozná Home Assistant.
class GenesisArea {
  const GenesisArea({
    required this.areaId,
    required this.name,
    required this.deviceIds,
  });

  /// Stabilné aj pri premenovaní miestnosti, takže sa naň dá viazať výber.
  final String areaId;
  final String name;
  final List<String> deviceIds;

  factory GenesisArea.fromJson(Map<String, dynamic> json) => GenesisArea(
        areaId: json['area_id'] as String,
        name: json['name'] as String,
        deviceIds: ((json['device_ids'] as List<dynamic>?) ?? const [])
            .map((id) => id as String)
            .toList(),
      );
}

/// Domácnosť, jej miestnosti a jej zariadenia z jedného okamihu.
class GenesisInventory {
  const GenesisInventory({
    required this.householdId,
    required this.householdName,
    required this.areas,
    required this.devices,
    required this.homeAssistantState,
    required this.roomsIncomplete,
  });

  final String householdId;

  /// Názov z Home Assistanta. Chýba, kým jednotka nenačíta konfiguráciu; panel
  /// si ho vtedy nesmie domyslieť.
  final String? householdName;
  final List<GenesisArea> areas;
  final List<GenesisDevice> devices;

  /// Stav prepojenia z tej istej odpovede. Bez neho sa prázdny inventár nedá
  /// odlíšiť od prázdnej domácnosti.
  final String homeAssistantState;

  /// Registre sa nepodarilo prečítať celé, takže miestnosti chýbajú aj vtedy,
  /// keď ich domácnosť má.
  final bool roomsIncomplete;

  factory GenesisInventory.fromJson(Map<String, dynamic> json) {
    final household = json['household'] as Map<String, dynamic>;
    final source = json['home_assistant'] as Map<String, dynamic>;
    return GenesisInventory(
      householdId: household['household_id'] as String,
      householdName: household['name'] as String?,
      areas: ((json['areas'] as List<dynamic>?) ?? const [])
          .map((area) => GenesisArea.fromJson(area as Map<String, dynamic>))
          .toList(),
      devices: ((json['devices'] as List<dynamic>?) ?? const [])
          .map((device) => GenesisDevice.fromJson(device as Map<String, dynamic>))
          .toList(),
      homeAssistantState: source['state'] as String,
      roomsIncomplete: source['rooms_incomplete'] as bool? ?? false,
    );
  }

  bool get isConnected => homeAssistantState == 'connected';

  /// Zariadenia, ktoré v Home Assistante miestnosť nemajú. Panel im dá vlastnú
  /// skupinu; jednotka pre ne miestnosť nevyrába, aby sa nedala zameniť za
  /// skutočnú.
  List<GenesisDevice> get devicesWithoutArea =>
      devices.where((device) => device.areaId == null).toList();
}

/// Dôkaz, na ktorý sa stav povelu odvoláva.
///
/// `provider_ack` je potvrdenie od Home Assistanta, že povel prijal;
/// `device_observation` je pozorovaná zmena na zariadení. To druhé je jediné,
/// čo hovorí o fyzickom svete, a preto sa v paneli nesmú zliať do jednej vety.
class GenesisEvidence {
  const GenesisEvidence({
    required this.kind,
    required this.reference,
    required this.at,
  });

  final String kind;
  final String reference;
  final DateTime? at;

  factory GenesisEvidence.fromJson(Map<String, dynamic> json) => GenesisEvidence(
        kind: json['kind'] as String,
        reference: json['reference'] as String? ?? '',
        at: DateTime.tryParse(json['at'] as String? ?? ''),
      );
}

/// Snapshot povelu z ledgeru, tak ako ho vydá `GET /v1/commands/{id}`.
class GenesisCommand {
  const GenesisCommand({
    required this.commandId,
    required this.correlationId,
    required this.deviceId,
    required this.status,
    required this.confirmationLevel,
    required this.statusChangedAt,
    required this.reason,
    required this.evidence,
  });

  final String commandId;

  /// Identifikátor, ktorým sa povel nahlasuje. Zadáva ho volajúci a prechádza
  /// každým záznamom v ledgeri, takže je to to jediné číslo, ktoré má človek
  /// pri neistom výsledku citovať.
  final String correlationId;
  final String deviceId;
  final String status;

  /// `none`, `provider` alebo `device`. Pri neistom výsledku prežije aj vtedy,
  /// keď dôkaz už v snapshote nie je — je to zvyšok toho, čo bolo predtým
  /// potvrdené, a bez neho by neistý povel vyzeral, že sa nestalo vôbec nič.
  final String confirmationLevel;
  final DateTime? statusChangedAt;
  final String? reason;
  final GenesisEvidence? evidence;

  factory GenesisCommand.fromJson(Map<String, dynamic> json) {
    final request = json['request'] as Map<String, dynamic>? ?? const {};
    final evidence = json['evidence'] as Map<String, dynamic>?;
    return GenesisCommand(
      commandId: request['command_id'] as String? ?? '',
      correlationId: request['correlation_id'] as String? ?? '',
      deviceId: request['device_id'] as String? ?? '',
      status: json['status'] as String,
      confirmationLevel: json['confirmation_level'] as String? ?? 'none',
      statusChangedAt: DateTime.tryParse(json['status_changed_at'] as String? ?? ''),
      reason: json['reason'] as String?,
      evidence: evidence == null ? null : GenesisEvidence.fromJson(evidence),
    );
  }

  /// Neistý výsledok. Nie je to „prebieha" ani „zlyhalo": nikto nevie, čo sa so
  /// zariadením stalo, a preto sa povel nesmie ponúknuť na zopakovanie.
  bool get isUncertain => status == 'unknown';

  /// Povel je ešte na ceste a výsledok môže prísť.
  bool get isInFlight => status == 'accepted' || status == 'sent';

  /// Jediné dva konečné stavy. `provider_confirmed` medzi ne nepatrí: Home
  /// Assistant povel prijal, ale o zariadení tým nepovedal nič.
  bool get isSettled => status == 'device_confirmed' || status == 'failed';

  /// Home Assistant potvrdil prijatie, či už je stav akýkoľvek neskôr.
  bool get providerAcknowledged =>
      confirmationLevel == 'provider' || confirmationLevel == 'device';
}

/// Stav prepojenia Genesis↔Home Assistant.
class GenesisHaLink {
  const GenesisHaLink({
    required this.state,
    required this.since,
    required this.lastInventoryAt,
    required this.lastInventoryDevices,
    required this.lastError,
  });

  /// `not_configured`, `connecting`, `connected` alebo `disconnected`.
  final String state;
  final DateTime? since;
  final DateTime? lastInventoryAt;
  final int lastInventoryDevices;

  /// Kategória posledného ukončenia sedenia, nikdy adresa ani token.
  final String? lastError;

  factory GenesisHaLink.fromJson(Map<String, dynamic> json) => GenesisHaLink(
        state: json['state'] as String,
        since: DateTime.tryParse(json['since'] as String? ?? ''),
        lastInventoryAt:
            DateTime.tryParse(json['last_inventory_at'] as String? ?? ''),
        lastInventoryDevices: json['last_inventory_devices'] as int? ?? 0,
        lastError: json['last_error'] as String?,
      );

  bool get isConnected => state == 'connected';

  /// Prepojenie nie je nastavené. Nie je to porucha a nemá sa tak zobraziť.
  bool get isUnconfigured => state == 'not_configured';
}

class GenesisDiagnostics {
  const GenesisDiagnostics({
    required this.version,
    required this.householdId,
    required this.homeAssistantConfigured,
    required this.homeAssistant,
  });

  final String version;
  final String householdId;
  final bool homeAssistantConfigured;
  final GenesisHaLink homeAssistant;

  factory GenesisDiagnostics.fromJson(Map<String, dynamic> json) {
    final unit = json['unit'] as Map<String, dynamic>;
    return GenesisDiagnostics(
      version: unit['version'] as String? ?? '',
      householdId: unit['household_id'] as String? ?? '',
      homeAssistantConfigured:
          unit['home_assistant_configured'] as bool? ?? false,
      homeAssistant: GenesisHaLink.fromJson(
        json['home_assistant'] as Map<String, dynamic>,
      ),
    );
  }
}

class GenesisApi {
  GenesisApi({required this.baseUrl, http.Client? client})
      : _client = client ?? http.Client(),
        _ownsClient = client == null;

  final Uri baseUrl;
  final http.Client _client;
  final bool _ownsClient;

  Uri _path(String path) => baseUrl.resolve(path);

  /// Telo odpovede ako JSON.
  ///
  /// Číta sa `bodyBytes` a dekóduje sa výslovne UTF-8, nie `response.body`.
  /// `response.body` sa riadi parametrom `charset` v hlavičke a bez neho padá na
  /// Latin-1, kým axum posiela `application/json` bez charsetu. JSON je podľa
  /// RFC 8259 UTF-8, takže `response.body` by z „Obývačky" urobil nečitateľnú
  /// zmes — a názvy miestností a zariadení sú to prvé, čo v slovenskej
  /// domácnosti diakritiku má.
  Map<String, dynamic> _asObject(http.Response response) =>
      jsonDecode(utf8.decode(response.bodyBytes)) as Map<String, dynamic>;

  List<dynamic> _asList(http.Response response) =>
      jsonDecode(utf8.decode(response.bodyBytes)) as List<dynamic>;

  Future<Map<String, dynamic>> _getJson(
    String path,
    String accessToken,
    String what,
  ) async {
    final response = await _client.get(
      _path(path),
      headers: {'Authorization': 'Bearer $accessToken'},
    ).timeout(const Duration(seconds: 5));
    if (response.statusCode != 200) {
      throw StateError('Genesis $what HTTP ${response.statusCode}');
    }
    return _asObject(response);
  }

  Future<GenesisPrincipal> me(String accessToken) async =>
      GenesisPrincipal.fromJson(await _getJson('v1/me', accessToken, 'identity'));

  /// Domácnosť, miestnosti a zariadenia jedným dotazom.
  ///
  /// Jedným preto, že miestnosti a zariadenia musia byť z toho istého okamihu:
  /// dvoma by sa dal dostať zoznam miestností a k nemu zariadenie ukazujúce do
  /// miestnosti, čo medzitým zanikla.
  Future<GenesisInventory> inventory(String readToken) async =>
      GenesisInventory.fromJson(
        await _getJson('v1/inventory', readToken, 'inventory'),
      );

  /// Detail jedného povelu z ledgeru. Toto je čítanie, nie zopakovanie povelu —
  /// pri neistom výsledku je to jediná bezpečná akcia.
  Future<GenesisCommand> command(String accessToken, String commandId) async =>
      GenesisCommand.fromJson(await _getJson(
        'v1/commands/${Uri.encodeComponent(commandId)}',
        accessToken,
        'command',
      ));

  /// Posledné povely domácnosti, najnovší prvý.
  Future<List<GenesisCommand>> commands(String accessToken,
      {int limit = 20}) async {
    final response = await _client.get(
      _path('v1/commands?limit=$limit'),
      headers: {'Authorization': 'Bearer $accessToken'},
    ).timeout(const Duration(seconds: 5));
    if (response.statusCode != 200) {
      throw StateError('Genesis ledger HTTP ${response.statusCode}');
    }
    return _asList(response)
        .map((item) => GenesisCommand.fromJson(item as Map<String, dynamic>))
        .toList();
  }

  Future<GenesisDiagnostics> diagnostics(String accessToken) async =>
      GenesisDiagnostics.fromJson(
        await _getJson('v1/diagnostics', accessToken, 'diagnostics'),
      );

  Future<GenesisCommand> setPower({
    required String writeToken,
    required String householdId,
    required String deviceId,
    required bool value,
  }) async {
    final key = 'panel-${DateTime.now().microsecondsSinceEpoch}';
    final response = await _client.post(
      _path('v1/commands'),
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
    return GenesisCommand.fromJson(_asObject(response));
  }

  void close() {
    if (_ownsClient) _client.close();
  }
}
