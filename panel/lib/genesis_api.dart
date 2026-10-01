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

/// Jednotka prístup odmietla.
///
/// Pri odobranej kreditíve je to normálny koniec života tokenu, nie porucha.
/// Preto má vlastný typ: panel na to musí reagovať inak než na výpadok siete —
/// token zahodiť a požiadať o nové párovanie, nie skúšať znova.
class GenesisUnauthorized implements Exception {
  const GenesisUnauthorized(this.what);

  final String what;

  @override
  String toString() => 'Genesis $what: prístup bol odmietnutý';
}

/// Rozbehnuté párovanie. `code` sa vracia **práve raz** — jednotka si ho drží
/// len ako odtlačok, takže keď ho vlastník stratí, musí vydať nový.
class GenesisPairing {
  const GenesisPairing({
    required this.pairingId,
    required this.role,
    required this.actorId,
    required this.expiresAt,
    required this.code,
  });

  final String pairingId;
  final String role;
  final String actorId;
  final DateTime? expiresAt;
  final String code;

  factory GenesisPairing.fromJson(Map<String, dynamic> json) => GenesisPairing(
        pairingId: json['pairing_id'] as String,
        role: json['role'] as String,
        actorId: json['actor_id'] as String,
        expiresAt: DateTime.tryParse(json['expires_at'] as String? ?? ''),
        code: json['code'] as String,
      );
}

/// Vydaná kreditíva. `token` sa rovnako ako kód vracia práve raz.
class GenesisIssuedCredential {
  const GenesisIssuedCredential({
    required this.credentialId,
    required this.householdId,
    required this.role,
    required this.actorId,
    required this.token,
  });

  final String credentialId;
  final String householdId;
  final String role;
  final String actorId;
  final String token;

  factory GenesisIssuedCredential.fromJson(Map<String, dynamic> json) =>
      GenesisIssuedCredential(
        credentialId: json['credential_id'] as String,
        householdId: json['household_id'] as String,
        role: json['role'] as String,
        actorId: json['actor_id'] as String,
        token: json['token'] as String,
      );
}

/// Čo o kreditíve smie vlastník vidieť. Token medzi tým nie je — jednotka ho
/// nevydá druhý raz ani vlastníkovi.
class GenesisCredential {
  const GenesisCredential({
    required this.credentialId,
    required this.householdId,
    required this.role,
    required this.actorId,
    required this.issuedAt,
    required this.lastUsedAt,
    required this.revokedAt,
    required this.revokedBy,
  });

  final String credentialId;
  final String householdId;
  final String role;
  final String actorId;
  final DateTime? issuedAt;
  final DateTime? lastUsedAt;
  final DateTime? revokedAt;
  final String? revokedBy;

  factory GenesisCredential.fromJson(Map<String, dynamic> json) =>
      GenesisCredential(
        credentialId: json['credential_id'] as String,
        householdId: json['household_id'] as String,
        role: json['role'] as String,
        actorId: json['actor_id'] as String,
        issuedAt: DateTime.tryParse(json['issued_at'] as String? ?? ''),
        lastUsedAt: DateTime.tryParse(json['last_used_at'] as String? ?? ''),
        revokedAt: DateTime.tryParse(json['revoked_at'] as String? ?? ''),
        revokedBy: json['revoked_by'] as String?,
      );

  bool get isRevoked => revokedAt != null;

  /// Kreditíva, ktorá ešte nebola použitá. Vlastník z toho vidí, či si člen kód
  /// vôbec uplatnil.
  bool get isUnused => lastUsedAt == null;
}

/// Jeden incident, ktorý Genesis o grante založil.
///
/// Otvorený incident nemá `resolvedAt`. Je to jediný údaj, ktorý hovorí „toto
/// nedopadlo a nikto to nepotvrdil" — preto sa nezlieva so stavom grantu.
class GenesisIncident {
  const GenesisIncident({
    required this.incidentId,
    required this.kind,
    required this.detail,
    required this.at,
    required this.resolvedAt,
  });

  final String incidentId;
  final String kind;
  final String detail;
  final DateTime? at;
  final DateTime? resolvedAt;

  factory GenesisIncident.fromJson(Map<String, dynamic> json) =>
      GenesisIncident(
        incidentId: json['incident_id'] as String,
        kind: json['kind'] as String,
        detail: json['detail'] as String,
        at: DateTime.tryParse(json['at'] as String? ?? ''),
        resolvedAt: DateTime.tryParse(json['resolved_at'] as String? ?? ''),
      );

  bool get isOpen => resolvedAt == null;
}

/// Posledný stav zariadenia, ktorý Genesis skutočne potvrdil dôkazom.
///
/// Toto je fyzický svet. Stav grantu je logická evidencia a tie dve veci sa
/// môžu rozchádzať — práve kvôli tomu je tu samostatný typ.
class GenesisConfirmedState {
  const GenesisConfirmedState({
    required this.commandId,
    required this.value,
    required this.confirmation,
    required this.at,
    required this.observedAt,
  });

  final String commandId;

  /// Hodnota, ktorú dôkaz potvrdil. Pri schopnosti `power` je to `bool`.
  final Object? value;

  /// `provider` alebo `device`. Iba druhé hovorí o fyzickom svete.
  final String confirmation;
  final DateTime? at;
  final DateTime? observedAt;

  factory GenesisConfirmedState.fromJson(Map<String, dynamic> json) =>
      GenesisConfirmedState(
        commandId: json['command_id'] as String,
        value: json['value'],
        confirmation: json['confirmation'] as String,
        at: DateTime.tryParse(json['at'] as String? ?? ''),
        observedAt: DateTime.tryParse(json['observed_at'] as String? ?? ''),
      );

  bool get isDeviceConfirmed => confirmation == 'device';
}

/// Časový prístup: jeden grant a všetko, čo o ňom Genesis vie.
class GenesisGrant {
  const GenesisGrant({
    required this.decisionId,
    required this.deviceId,
    required this.capabilityId,
    required this.grantedValue,
    required this.expiresAt,
    required this.state,
    required this.unlockConfirmed,
    required this.updatedAt,
    required this.requiredConfirmation,
    required this.closeAttempts,
    required this.lastConfirmed,
    required this.openIncidents,
  });

  /// Identifikátor rozhodnutia, ktoré prístup otvorilo. Je to zároveň kľúč,
  /// ktorým sa dá vyžiadať zosúladenie.
  final String decisionId;
  final String deviceId;
  final String capabilityId;

  /// Hodnota, na ktorú bol prístup otvorený. Zatvára sa jej opakom.
  final bool grantedValue;
  final DateTime? expiresAt;

  /// `granted`, `active`, `unlock_failed`, `relocked`, `relock_pending` alebo
  /// `superseded`. Je to logický stav evidencie, nie stav zariadenia.
  final String state;

  /// Nepravda znamená, že unlock nedosiahol vyžadovanú úroveň potvrdenia.
  final bool unlockConfirmed;
  final DateTime? updatedAt;

  /// Čo rozhodnutie vyžadovalo ako dôkaz: `provider` alebo `device`.
  final String requiredConfirmation;
  final int closeAttempts;
  final GenesisConfirmedState? lastConfirmed;
  final List<GenesisIncident> openIncidents;

  factory GenesisGrant.fromJson(Map<String, dynamic> json) {
    final grant = json['grant'] as Map<String, dynamic>;
    final confirmed = json['last_confirmed'] as Map<String, dynamic>?;
    return GenesisGrant(
      decisionId: grant['decision_id'] as String,
      deviceId: grant['device_id'] as String,
      capabilityId: grant['capability_id'] as String,
      grantedValue: grant['granted_value'] as bool,
      expiresAt: DateTime.tryParse(grant['expires_at'] as String? ?? ''),
      state: grant['state'] as String,
      unlockConfirmed: grant['unlock_confirmed'] as bool? ?? false,
      updatedAt: DateTime.tryParse(grant['updated_at'] as String? ?? ''),
      requiredConfirmation: json['required_confirmation'] as String,
      closeAttempts: json['close_attempts'] as int? ?? 0,
      lastConfirmed: confirmed == null
          ? null
          : GenesisConfirmedState.fromJson(confirmed),
      openIncidents: ((json['open_incidents'] as List<dynamic>?) ?? const [])
          .map((item) => GenesisIncident.fromJson(item as Map<String, dynamic>))
          .toList(),
    );
  }

  /// Stavy, v ktorých môže byť zariadenie stále otvorené. Zhoduje sa s
  /// `GrantState::is_open` na jednotke.
  bool get isOpen =>
      state == 'granted' || state == 'active' || state == 'relock_pending';

  /// Relock skončil neisto a stav čaká na zosúladenie.
  bool get isRelockPending => state == 'relock_pending';

  /// Okno uplynulo. Panel to počíta sám, pretože jednotka vracia čas, nie
  /// príznak — a čas sa dá prekresliť bez ďalšieho dotazu.
  bool expiredAt(DateTime now) =>
      expiresAt != null && !expiresAt!.isAfter(now);

  /// Či by zosúladenie vôbec malo čo robiť. Zhoduje sa s `grant::is_reconcilable`
  /// na jednotke; panel podľa toho tlačidlo deaktivuje, nech nevyzerá, že
  /// nefunguje.
  bool isReconcilable(DateTime now) {
    if (state == 'granted' || state == 'relock_pending') return true;
    if (state == 'active') return expiredAt(now);
    return false;
  }
}

/// Čo urobilo vyžiadané zosúladenie, aj s tým, čo o grante vieme potom.
class GenesisReconcileReceipt {
  const GenesisReconcileReceipt({required this.outcome, required this.grant});

  /// `settled`, `attempted`, `unchanged`, `not_open` alebo `not_due`.
  ///
  /// `settled` a `attempted` sa zámerne nezlievajú: prvé znamená dokázateľne
  /// zatvorený prístup, druhé že sa o to Genesis pokúsil a dôkaz nemá.
  final String outcome;
  final GenesisGrant grant;

  factory GenesisReconcileReceipt.fromJson(Map<String, dynamic> json) =>
      GenesisReconcileReceipt(
        outcome: json['outcome'] as String,
        grant: GenesisGrant.fromJson(json['access'] as Map<String, dynamic>),
      );
}

/// Typovaný intent, ktorý jednotka z prepisu zložila.
///
/// Nesie zariadenie, nie to, čo bolo povedané. Práve to je zmysel prekladu
/// prepisu na intent: ďalej v systéme už nikto nepracuje s textom.
class GenesisVoiceIntent {
  const GenesisVoiceIntent({required this.deviceId, required this.value});

  final String deviceId;
  final bool value;

  factory GenesisVoiceIntent.fromJson(Map<String, dynamic> json) =>
      GenesisVoiceIntent(
        deviceId: json['device_id'] as String,
        value: json['value'] as bool,
      );
}

/// Vysvetlenie výsledku. `code` je pre klienta, `message` pre človeka.
class GenesisExplanation {
  const GenesisExplanation({required this.code, required this.message});

  final String code;
  final String message;

  factory GenesisExplanation.fromJson(Map<String, dynamic> json) =>
      GenesisExplanation(
        code: json['code'] as String,
        message: json['message'] as String,
      );
}

/// Výsledok hlasového povelu.
///
/// Štyri stavy a **ani jeden z nich nie je chyba klienta**. Jednotka vracia
/// nejednoznačný a zamietnutý povel s 422, čo je stále odpoveď o domácnosti, nie
/// porucha — preto to tu nie je výnimka, ale hodnota. Keby to bola výnimka,
/// panel by nemal čo zobraziť práve v tých prípadoch, kde človek najviac
/// potrebuje vedieť prečo.
class GenesisVoiceOutcome {
  const GenesisVoiceOutcome({
    required this.outcome,
    this.intent,
    this.command,
    this.explanation,
    this.confirmationId,
    this.expiresAt,
    this.reason,
    this.candidates = const [],
    this.message,
  });

  /// `executed`, `confirmation_required`, `unclear` alebo `refused`.
  final String outcome;
  final GenesisVoiceIntent? intent;

  /// Povel v ledgeri. Len pri `executed` — inak sa nič nevykonalo a nič
  /// nevzniklo.
  final GenesisCommand? command;
  final GenesisExplanation? explanation;

  /// Identifikátor potvrdenia citlivej akcie. Platí raz a krátko.
  final String? confirmationId;
  final DateTime? expiresAt;

  /// Prečo sa povel nedal pochopiť (`unclear`).
  final String? reason;

  /// Zariadenia, medzi ktorými sa jednotka nerozhodla.
  final List<String> candidates;
  final String? message;

  factory GenesisVoiceOutcome.fromJson(Map<String, dynamic> json) {
    final intent = json['intent'];
    final command = json['command'] as Map<String, dynamic>?;
    final explanation = json['explanation'] as Map<String, dynamic>?;
    return GenesisVoiceOutcome(
      outcome: json['outcome'] as String,
      // `intent` je vnorený objekt s vlastným diskriminátorom `intent`, nie
      // reťazec. Tvar je pripnutý testom `the_outcome_names_itself_on_the_wire`
      // v `core/src/voice.rs`, aby sa zmena kontraktu neukázala až tu.
      intent: intent is Map<String, dynamic>
          ? GenesisVoiceIntent.fromJson(intent)
          : null,
      command: command == null ? null : GenesisCommand.fromJson(command),
      explanation:
          explanation == null ? null : GenesisExplanation.fromJson(explanation),
      confirmationId: json['confirmation_id'] as String?,
      expiresAt: DateTime.tryParse(json['expires_at'] as String? ?? ''),
      reason: json['reason'] as String?,
      candidates: ((json['candidates'] as List<dynamic>?) ?? const [])
          .map((item) => item as String)
          .toList(),
      message: json['message'] as String?,
    );
  }

  bool get wasExecuted => outcome == 'executed';

  /// Citlivá akcia. **Nič sa nevykonalo** a čaká sa na výslovné potvrdenie.
  bool get needsConfirmation => outcome == 'confirmation_required';
  bool get isUnclear => outcome == 'unclear';
  bool get wasRefused => outcome == 'refused';

  /// Či sa niečo stalo so zariadením. Pre tri zo štyroch stavov je to nepravda a
  /// panel to nesmie zliať do „neúspechu": nevykonané z opatrnosti a nevykonané
  /// pre nepochopenie sú pre človeka dve rôzne veci.
  bool get touchedTheHouse => wasExecuted;
}

/// Záznam auditu hlasovej akcie.
///
/// Zámerne bez prepisu a bez identifikátora potvrdenia: audit dokladá
/// rozhodnutie, nie obsah toho, čo bolo povedané.
class GenesisVoiceAuditEvent {
  const GenesisVoiceAuditEvent({
    required this.eventId,
    required this.actorId,
    required this.decision,
    required this.reason,
    required this.deviceId,
    required this.commandId,
    required this.at,
  });

  final String eventId;
  final String actorId;

  /// `executed`, `refused` alebo `awaiting_confirmation`.
  final String decision;
  final String reason;
  final String? deviceId;
  final String? commandId;
  final DateTime? at;

  factory GenesisVoiceAuditEvent.fromJson(Map<String, dynamic> json) =>
      GenesisVoiceAuditEvent(
        eventId: json['event_id'] as String,
        actorId: json['actor_id'] as String,
        decision: json['decision'] as String,
        reason: json['reason'] as String,
        deviceId: json['device_id'] as String?,
        commandId: json['command_id'] as String?,
        at: DateTime.tryParse(json['at'] as String? ?? ''),
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

  /// Telo odpovede ako JSON.
  ///
  /// Číta sa `bodyBytes` a dekóduje sa výslovne UTF-8, nie `response.body`.
  /// `response.body` sa riadi parametrom `charset` v hlavičke a bez neho padá na
  /// Latin-1, kým axum posiela `application/json` bez charsetu. JSON je podľa
  /// RFC 8259 UTF-8, takže `response.body` by z „Obývačky" urobil nečitateľnú
  /// zmes — a názvy miestností a zariadení sú to prvé, čo v slovenskej
  /// domácnosti diakritiku má.
  /// 401 a 403 majú vlastný typ. Pre panel je to tá istá vec — token, ktorý
  /// nefunguje — a reaguje sa na ňu zahodením tokenu, nie opakovaním.
  void _refuseIfUnauthorized(http.Response response, String what) {
    if (response.statusCode == 401 || response.statusCode == 403) {
      throw GenesisUnauthorized(what);
    }
  }

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
    _refuseIfUnauthorized(response, what);
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
    _refuseIfUnauthorized(response, 'ledger');
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
    _refuseIfUnauthorized(response, 'command');
    if (response.statusCode != 200) {
      throw StateError('Genesis command HTTP ${response.statusCode}');
    }
    return GenesisCommand.fromJson(_asObject(response));
  }

  /// Vlastník vydá párovací kód. Kód je v odpovedi raz a panel ho má raz
  /// zobraziť; jednotka si drží len jeho odtlačok.
  Future<GenesisPairing> createPairing({
    required String ownerToken,
    required String householdId,
    required String role,
    required String actorId,
  }) async {
    final response = await _client.post(
      _path('v1/pairings'),
      headers: {
        'Authorization': 'Bearer $ownerToken',
        'Content-Type': 'application/json',
      },
      body: jsonEncode({
        'household_id': householdId,
        'role': role,
        'actor_id': actorId,
      }),
    ).timeout(const Duration(seconds: 8));
    _refuseIfUnauthorized(response, 'pairing');
    if (response.statusCode != 201) {
      throw StateError('Genesis pairing HTTP ${response.statusCode}');
    }
    return GenesisPairing.fromJson(_asObject(response));
  }

  /// Člen kód uplatní. Tento jediný hovor nepotrebuje token, pretože kód sám je
  /// oprávnenie — a práve preto ide v tele, nie v adrese.
  Future<GenesisIssuedCredential> redeemPairing({
    required String householdId,
    required String code,
  }) async {
    final response = await _client.post(
      _path('v1/pairings/redeem'),
      headers: {'Content-Type': 'application/json'},
      body: jsonEncode({'household_id': householdId, 'code': code}),
    ).timeout(const Duration(seconds: 8));
    if (response.statusCode != 200) {
      throw StateError('Genesis redeem HTTP ${response.statusCode}');
    }
    return GenesisIssuedCredential.fromJson(_asObject(response));
  }

  Future<List<GenesisCredential>> credentials(String ownerToken) async {
    final response = await _client.get(
      _path('v1/credentials'),
      headers: {'Authorization': 'Bearer $ownerToken'},
    ).timeout(const Duration(seconds: 5));
    _refuseIfUnauthorized(response, 'credentials');
    if (response.statusCode != 200) {
      throw StateError('Genesis credentials HTTP ${response.statusCode}');
    }
    return _asList(response)
        .map((item) => GenesisCredential.fromJson(item as Map<String, dynamic>))
        .toList();
  }

  /// Odoberie prístup. Identifikátor kreditívy nie je tajomstvo, takže v ceste
  /// byť môže — na rozdiel od tokenu a kódu, ktoré v adrese nebudú nikdy.
  Future<void> revokeCredential({
    required String ownerToken,
    required String credentialId,
  }) async {
    final response = await _client.delete(
      _path('v1/credentials/${Uri.encodeComponent(credentialId)}'),
      headers: {'Authorization': 'Bearer $ownerToken'},
    ).timeout(const Duration(seconds: 8));
    _refuseIfUnauthorized(response, 'revocation');
    if (response.statusCode != 204) {
      throw StateError('Genesis revocation HTTP ${response.statusCode}');
    }
  }

  /// Aktívne a nedovrené časové prístupy domácnosti.
  ///
  /// Čítať to smie každá rola, ktorá má token: kto v domácnosti žije, má vedieť,
  /// že sa mu niečo zamyká samo.
  Future<List<GenesisGrant>> access(String accessToken) async {
    final response = await _client.get(
      _path('v1/access'),
      headers: {'Authorization': 'Bearer $accessToken'},
    ).timeout(const Duration(seconds: 5));
    _refuseIfUnauthorized(response, 'access');
    if (response.statusCode != 200) {
      throw StateError('Genesis access HTTP ${response.statusCode}');
    }
    return _asList(response)
        .map((item) => GenesisGrant.fromJson(item as Map<String, dynamic>))
        .toList();
  }

  /// Vyžiada jeden prechod zosúladenia nad jedným grantom.
  ///
  /// Jednotka týmto nedokáže nič otvoriť ani predĺžiť — posiela výlučne
  /// uzatváraciu hodnotu — a platné okno neskráti. Dlhší časový limit než pri
  /// čítaní je preto, že za tým môže byť skutočný povel na zariadenie.
  ///
  /// Identifikátor rozhodnutia nie je tajomstvo, takže v ceste byť môže.
  Future<GenesisReconcileReceipt> reconcileAccess({
    required String ownerToken,
    required String decisionId,
  }) async {
    final response = await _client.post(
      _path('v1/access/${Uri.encodeComponent(decisionId)}/reconcile'),
      headers: {'Authorization': 'Bearer $ownerToken'},
    ).timeout(const Duration(seconds: 15));
    _refuseIfUnauthorized(response, 'reconciliation');
    if (response.statusCode != 200) {
      throw StateError('Genesis reconciliation HTTP ${response.statusCode}');
    }
    return GenesisReconcileReceipt.fromJson(_asObject(response));
  }

  /// Odošle prepis a vráti, čo s ním jednotka urobila.
  ///
  /// 200, 202 aj 422 sú **odpovede**, nie chyby: vykonané, čaká na potvrdenie,
  /// nepochopené a zamietnuté. Preto sa telo číta pri všetkých. Keby 422
  /// vyhodilo výnimku, panel by nemal čo povedať práve tam, kde človek
  /// potrebuje dôvod.
  ///
  /// Prepis ide v tele, nikdy v adrese — je to obsah toho, čo niekto povedal vo
  /// svojej domácnosti, a adresy sa logujú na miestach, ktoré nemáme v rukách.
  Future<GenesisVoiceOutcome> speak({
    required String accessToken,
    required String householdId,
    required String transcript,
    required bool storeTranscript,
  }) async {
    final key = 'panel-voice-${DateTime.now().microsecondsSinceEpoch}';
    final response = await _client.post(
      _path('v1/voice/commands'),
      headers: {
        'Authorization': 'Bearer $accessToken',
        'Content-Type': 'application/json',
      },
      body: jsonEncode({
        'household_id': householdId,
        'transcript': transcript,
        'store_transcript': storeTranscript,
        'idempotency_key': key,
        'correlation_id': key,
      }),
    ).timeout(const Duration(seconds: 15));
    _refuseIfUnauthorized(response, 'voice');
    return _voiceOutcome(response, 'voice');
  }

  /// Potvrdí citlivú akciu.
  ///
  /// Identifikátor potvrdenia je jednorazové oprávnenie vykonať konkrétnu akciu,
  /// takže ide v tele ako párovací kód — nie v adrese.
  Future<GenesisVoiceOutcome> confirmSpoken({
    required String accessToken,
    required String householdId,
    required String confirmationId,
  }) async {
    final response = await _client.post(
      _path('v1/voice/confirmations'),
      headers: {
        'Authorization': 'Bearer $accessToken',
        'Content-Type': 'application/json',
      },
      body: jsonEncode({
        'household_id': householdId,
        'confirmation_id': confirmationId,
      }),
    ).timeout(const Duration(seconds: 15));
    _refuseIfUnauthorized(response, 'confirmation');
    return _voiceOutcome(response, 'confirmation');
  }

  /// Telo hlasovej odpovede. 200, 202 a 422 nesú výsledok; čokoľvek iné je
  /// skutočná chyba a nie je sa čoho držať.
  GenesisVoiceOutcome _voiceOutcome(http.Response response, String what) {
    if (response.statusCode != 200 &&
        response.statusCode != 202 &&
        response.statusCode != 422) {
      throw StateError('Genesis $what HTTP ${response.statusCode}');
    }
    return GenesisVoiceOutcome.fromJson(_asObject(response));
  }

  /// Audit hlasových rozhodnutí, najnovšie prvé. Číta ho každá rola.
  Future<List<GenesisVoiceAuditEvent>> voiceAudit(String accessToken) async {
    final response = await _client.get(
      _path('v1/voice/audit'),
      headers: {'Authorization': 'Bearer $accessToken'},
    ).timeout(const Duration(seconds: 5));
    _refuseIfUnauthorized(response, 'voice audit');
    if (response.statusCode != 200) {
      throw StateError('Genesis voice audit HTTP ${response.statusCode}');
    }
    return _asList(response)
        .map((item) =>
            GenesisVoiceAuditEvent.fromJson(item as Map<String, dynamic>))
        .toList();
  }

  void close() {
    if (_ownsClient) _client.close();
  }
}
