import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/foundation.dart';
import 'package:http/http.dart' as http;

import 'genesis_api.dart';
import 'theme.dart';
import 'token_store.dart';

void main() => runApp(const GenesisApp());

String genesisDefaultApiUrl() {
  const configured = String.fromEnvironment('GENESIS_API_URL');
  if (configured.isNotEmpty) return configured;
  return kIsWeb ? Uri.base.resolve('.').toString() : 'http://localhost:8765';
}

class GenesisApp extends StatelessWidget {
  const GenesisApp({super.key, this.client, this.tokenStore});

  /// Vymeniteľný HTTP klient. Prezentácia neistého výsledku a výpadku Home
  /// Assistanta sa inak nedá otestovať bez skutočnej jednotky, a práve tie dve
  /// veci sa nesmú zobraziť ako úspech.
  final http.Client? client;

  /// Kde žije vydaný token. V testoch v pamäti; inak podľa platformy.
  final GenesisTokenStore? tokenStore;

  @override
  Widget build(BuildContext context) => MaterialApp(
        title: 'OS Genesis',
        debugShowCheckedModeBanner: false,
        // The app is adaptive and the panel lives inside Home Assistant, so it
        // follows the system rather than forcing one appearance.
        theme: elysiumTheme(Brightness.light),
        darkTheme: elysiumTheme(Brightness.dark),
        themeMode: ThemeMode.system,
        home: GenesisHome(client: client, tokenStore: tokenStore),
      );
}

enum ConnectionStatus { checking, online, offline }

/// Čo je v paneli vybrané.
///
/// „Bez miestnosti" nie je miestnosť a nemá identifikátor, takže sa nedá vyjadriť
/// len ako `areaId`; preto rozsah a identifikátor idú zvlášť.
enum AreaScope { whole, area, withoutArea }

class GenesisHome extends StatefulWidget {
  const GenesisHome({super.key, this.client, this.tokenStore});

  final http.Client? client;
  final GenesisTokenStore? tokenStore;

  @override
  State<GenesisHome> createState() => _GenesisHomeState();
}

class _GenesisHomeState extends State<GenesisHome> {
  final _url = TextEditingController(text: genesisDefaultApiUrl());
  final _accessToken = TextEditingController();
  GenesisPrincipal? _principal;
  late final http.Client _client = widget.client ?? http.Client();
  late final bool _ownsClient = widget.client == null;
  late final GenesisTokenStore _tokenStore = widget.tokenStore ?? SecureTokenStore();
  Timer? _timer;
  ConnectionStatus _status = ConnectionStatus.checking;
  GenesisInventory? _inventory;
  AreaScope _scope = AreaScope.whole;
  String? _areaId;
  String? _inventoryError;
  GenesisCommand? _lastCommand;
  String? _commandMessage;
  String? _pendingDeviceId;
  bool _readingCommand = false;
  List<GenesisCommand> _commands = [];
  String? _ledgerError;
  GenesisDiagnostics? _diagnostics;
  String? _diagnosticsError;
  final _pairingActor = TextEditingController();
  final _redeemCode = TextEditingController();
  final _redeemHousehold = TextEditingController();
  String _pairingRole = 'member';
  /// Vydaný kód. Drží sa, kým ho vlastník nezatvorí — zobraziť sa dá raz.
  GenesisPairing? _issuedCode;
  List<GenesisCredential> _credentials = [];
  String? _accessError;
  String? _accessNotice;
  bool _busyWithAccess = false;

  @override
  void initState() {
    super.initState();
    _checkHealth();
    _restoreToken();
    _timer = Timer.periodic(const Duration(seconds: 15), (_) {
      _checkHealth();
      if (_accessToken.text.isNotEmpty) _refreshDevices();
    });
  }

  /// Token z úložiska. Bez neho by člen po obnovení stránky o prístup prišiel a
  /// nový kód mu nemá kto vydať — párovací kód sa dá uplatniť práve raz.
  Future<void> _restoreToken() async {
    final stored = await _tokenStore.read();
    if (!mounted || stored == null || stored.isEmpty) return;
    setState(() => _accessToken.text = stored);
    await _refreshDevices();
  }

  /// Token, ktorý jednotka odmietla, sa zahodí.
  ///
  /// Odobraná kreditíva nie je chyba siete a opakovanie ju nevráti, takže sa
  /// token vymaže z úložiska aj z poľa. Panel potom povie jedinú vec, ktorá
  /// pomôže: požiadať vlastníka o nový kód.
  Future<void> _accessWasRefused() async {
    await _tokenStore.clear();
    if (!mounted) return;
    setState(() {
      _principal = null;
      _inventory = null;
      _diagnostics = null;
      _credentials = [];
      _commands = [];
      _accessToken.clear();
      _accessNotice =
          'Prístup bol odobraný alebo vypršal. Požiadaj vlastníka o nový párovací kód.';
    });
  }

  Uri? _baseUrl() {
    final base = Uri.tryParse(_url.text.trim());
    if (base == null ||
        (base.scheme != 'http' && base.scheme != 'https') ||
        base.host.isEmpty) {
      return null;
    }
    return base;
  }

  Future<void> _checkHealth() async {
    final base = _baseUrl();
    if (base == null) {
      if (mounted) setState(() => _status = ConnectionStatus.offline);
      return;
    }
    if (mounted) setState(() => _status = ConnectionStatus.checking);
    try {
      final response = await _client
          .get(base.resolve('health'))
          .timeout(const Duration(seconds: 4));
      if (mounted) {
        setState(() => _status = response.statusCode == 200
            ? ConnectionStatus.online
            : ConnectionStatus.offline);
      }
    } catch (_) {
      if (mounted) setState(() => _status = ConnectionStatus.offline);
    }
  }

  Future<void> _refreshDevices() async {
    final base = _baseUrl();
    if (base == null || _accessToken.text.isEmpty) {
      setState(() => _inventoryError = 'Zadajte adresu a prístupový token.');
      return;
    }
    try {
      final api = GenesisApi(baseUrl: base, client: _client);
      final principal = await api.me(_accessToken.text);
      final inventory = await api.inventory(_accessToken.text);
      if (mounted) {
        setState(() {
          _principal = principal;
          _inventory = inventory;
          _inventoryError = null;
          _accessNotice = null;
          _keepSelectionValid(inventory);
        });
      }
      if (principal.role == 'owner') await _refreshCredentials();
    } on GenesisUnauthorized {
      await _accessWasRefused();
      return;
    } catch (_) {
      if (mounted) {
        setState(() {
          _principal = null;
          _inventory = null;
          _inventoryError = 'Inventár nie je dostupný. Skontrolujte spojenie a prístup.';
        });
      }
    }
    await _refreshOverview();
  }

  /// Vybraná miestnosť mohla medzitým zaniknúť — niekto ju v Home Assistante
  /// zmazal alebo z nej vzal posledné zariadenie. Držať výber na nej by
  /// znamenalo ukazovať prázdno a tvrdiť, že miestnosť existuje.
  void _keepSelectionValid(GenesisInventory inventory) {
    if (_scope != AreaScope.area) return;
    final stillThere = inventory.areas.any((area) => area.areaId == _areaId);
    if (!stillThere) {
      _scope = AreaScope.whole;
      _areaId = null;
    }
  }

  /// Zariadenia podľa vybraného rozsahu.
  List<GenesisDevice> get _visibleDevices {
    final devices = _inventory?.devices ?? const <GenesisDevice>[];
    return switch (_scope) {
      AreaScope.whole => devices,
      AreaScope.area =>
        devices.where((device) => device.areaId == _areaId).toList(),
      AreaScope.withoutArea =>
        devices.where((device) => device.areaId == null).toList(),
    };
  }

  /// Názov domácnosti. Pred pripojením netvrdí nič konkrétne; potom je to názov
  /// z Home Assistanta, a keď ho jednotka nemá, aspoň identifikátor domácnosti.
  String get _householdLabel {
    final inventory = _inventory;
    if (inventory == null) return 'Domácnosť';
    return inventory.householdName ?? inventory.householdId;
  }

  String get _scopeLabel => switch (_scope) {
        AreaScope.whole => 'Celá domácnosť',
        AreaScope.withoutArea => 'Bez miestnosti',
        AreaScope.area => _inventory?.areas
                .firstWhere(
                  (area) => area.areaId == _areaId,
                  orElse: () => const GenesisArea(
                    areaId: '',
                    name: 'Celá domácnosť',
                    deviceIds: [],
                  ),
                )
                .name ??
            'Celá domácnosť',
      };

  /// Diagnostika a ledger. Obe sa načítajú aj vtedy, keď inventár zlyhal —
  /// práve vtedy sú najviac na niečo, pretože povedia, či jednotka nevidí k
  /// Home Assistantovi alebo či zlyhalo niečo iné.
  Future<void> _refreshOverview() async {
    final base = _baseUrl();
    if (base == null || _accessToken.text.isEmpty) return;
    final api = GenesisApi(baseUrl: base, client: _client);
    try {
      final diagnostics = await api.diagnostics(_accessToken.text);
      if (mounted) {
        setState(() {
          _diagnostics = diagnostics;
          _diagnosticsError = null;
        });
      }
    } on GenesisUnauthorized {
      await _accessWasRefused();
      return;
    } catch (_) {
      if (mounted) {
        setState(() {
          _diagnostics = null;
          _diagnosticsError =
              'Diagnostika nie je dostupná, takže stav prepojenia na Home Assistant nie je známy.';
        });
      }
    }
    try {
      final commands = await api.commands(_accessToken.text);
      if (mounted) {
        setState(() {
          _commands = commands;
          _ledgerError = null;
        });
      }
    } catch (_) {
      // Prázdny zoznam by tvrdil, že povely nie sú. Nevieme to — vieme len, že
      // sa nedali prečítať.
      if (mounted) {
        setState(() {
          _commands = [];
          _ledgerError = 'Ledger sa nepodarilo prečítať. Posledné povely nie sú známe.';
        });
      }
    }
  }

  /// Znovu prečíta stav povelu. Je to čítanie z ledgeru, nie druhé odoslanie —
  /// pri neistom výsledku je to jediná akcia, ktorá sa smie ponúknuť.
  Future<void> _readCommandAgain() async {
    final base = _baseUrl();
    final command = _lastCommand;
    if (base == null || command == null || _accessToken.text.isEmpty) return;
    setState(() => _readingCommand = true);
    try {
      final api = GenesisApi(baseUrl: base, client: _client);
      final fresh = await api.command(_accessToken.text, command.commandId);
      if (mounted) {
        setState(() {
          _lastCommand = fresh;
          _commandMessage = null;
        });
      }
    } catch (_) {
      if (mounted) {
        setState(() => _commandMessage =
            'Stav povelu sa nepodarilo prečítať. Výsledok zostáva taký, aký je uvedený.');
      }
    } finally {
      if (mounted) setState(() => _readingCommand = false);
    }
    await _refreshDevices();
  }

  /// Neistý povel na tomto zariadení, ak je posledný povel práve taký.
  ///
  /// Kým tam je, prepínač sa neponúka. Slepé zopakovanie je presne to, čo sa pri
  /// neznámom výsledku nesmie stať: nikto nevie, či prvý povel prešiel, takže
  /// druhý môže zariadenie prepnúť naopak.
  GenesisCommand? _uncertainCommandFor(String deviceId) {
    final last = _lastCommand;
    for (final command in [if (last != null) last, ..._commands]) {
      if (command.deviceId == deviceId) {
        return command.isUncertain ? command : null;
      }
    }
    return null;
  }

  Future<void> _setPower(GenesisDevice device) async {
    final base = _baseUrl();
    if (base == null || _principal?.canControlDevices != true || device.power == null) {
      setState(() => _commandMessage = 'Táto rola nemôže ovládať zariadenie.');
      return;
    }
    final uncertain = _uncertainCommandFor(device.id);
    if (uncertain != null) {
      setState(() => _commandMessage =
          'Posledný povel na toto zariadenie skončil neisto (referencia ${uncertain.correlationId}). '
          'Zopakovanie sa neponúka, kým sa stav nevyjasní — najprv obnovte stav.');
      return;
    }
    setState(() {
      _pendingDeviceId = device.id;
      _commandMessage = null;
      _lastCommand = null;
    });
    try {
      final api = GenesisApi(baseUrl: base, client: _client);
      final result = await api.setPower(
        writeToken: _accessToken.text,
        householdId: _principal!.householdId,
        deviceId: device.id,
        value: !device.power!,
      );
      if (mounted) {
        setState(() => _lastCommand = result);
        await _refreshDevices();
      }
    } on GenesisUnauthorized {
      await _accessWasRefused();
    } catch (_) {
      if (mounted) {
        setState(() => _commandMessage =
            'Výsledok povelu je neistý. Pred opakovaním skontrolujte Genesis ledger.');
      }
    } finally {
      if (mounted) setState(() => _pendingDeviceId = null);
    }
  }

  @override
  void dispose() {
    _timer?.cancel();
    if (_ownsClient) _client.close();
    _url.dispose();
    _accessToken.dispose();
    _pairingActor.dispose();
    _redeemCode.dispose();
    _redeemHousehold.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final wide = MediaQuery.sizeOf(context).width >= 800;
    final colors = ElysiumColors.of(context);
    return Scaffold(
      body: SafeArea(
        child: Row(
          children: [
            if (wide)
              Container(
                width: 264,
                decoration: BoxDecoration(
                  color: colors.surface,
                  border: Border(right: BorderSide(color: colors.border)),
                ),
                child: _navigation(wide),
              ),
            Expanded(
              child: Column(
                children: [
                  _header(wide),
                  Expanded(child: _content(wide)),
                ],
              ),
            ),
          ],
        ),
      ),
      drawer: wide
          ? null
          : Drawer(
              backgroundColor: colors.surface,
              child: SafeArea(child: _navigation(wide)),
            ),
    );
  }

  /// Wordmark and live status, instead of a stock app bar.
  Widget _header(bool wide) {
    final colors = ElysiumColors.of(context);
    return Container(
      padding: EdgeInsets.symmetric(
        horizontal: wide ? ElysiumLayout.screenPadding + 12 : ElysiumLayout.screenPadding,
        vertical: 16,
      ),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: colors.border)),
      ),
      child: Row(
        children: [
          if (!wide)
            Builder(
              builder: (context) => Padding(
                padding: const EdgeInsets.only(right: 8),
                child: IconButton(
                  onPressed: () => Scaffold.of(context).openDrawer(),
                  icon: const Icon(Icons.menu),
                  tooltip: 'Navigácia',
                ),
              ),
            ),
          const ElysiumWordmark('GENESIS'),
          const Spacer(),
          _statusChip(),
        ],
      ),
    );
  }

  /// Content is capped and centred, the way the app keeps cards off the edges on
  /// iPad and in wide windows.
  Widget _content(bool wide) => ListView(
        padding: EdgeInsets.symmetric(
          horizontal: wide ? ElysiumLayout.screenPadding + 12 : ElysiumLayout.screenPadding,
          vertical: ElysiumLayout.sectionSpacing + 4,
        ),
        children: [
          Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: ElysiumLayout.maximumContentWidth),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  _hero(),
                  const SizedBox(height: ElysiumLayout.sectionSpacing + 8),
                  _devicesSection(),
                  const SizedBox(height: ElysiumLayout.sectionSpacing + 8),
                  _diagnosticsSection(),
                  const SizedBox(height: ElysiumLayout.sectionSpacing + 8),
                  _accessSection(),
                  const SizedBox(height: ElysiumLayout.sectionSpacing + 8),
                  _ledgerSection(),
                  const SizedBox(height: ElysiumLayout.sectionSpacing + 8),
                  _connectionSection(),
                ],
              ),
            ),
          ),
        ],
      );

  Widget _hero() {
    final theme = Theme.of(context);
    final colors = ElysiumColors.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Text(_householdLabel, style: theme.textTheme.labelMedium),
            if (_principal != null) ...[
              Text('  ·  ', style: theme.textTheme.labelMedium),
              Text('Rola: ${_principal!.role}', style: theme.textTheme.labelMedium),
            ],
          ],
        ),
        const SizedBox(height: 6),
        Text(_scopeLabel, style: theme.textTheme.displaySmall),
        const SizedBox(height: 10),
        Container(
          height: 3,
          width: 56,
          decoration: BoxDecoration(
            gradient: colors.goldGradient,
            borderRadius: BorderRadius.circular(999),
          ),
        ),
      ],
    );
  }

  Widget _devicesSection() {
    final theme = Theme.of(context);
    final visible = _visibleDevices;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const ElysiumSectionLabel('Zariadenia'),
        const SizedBox(height: 10),
        Text(
          'Inventár domácnosti z Home Assistanta. Miestnosti aj ich priradenie '
          'sú tie, ktoré sú nastavené tam.',
          style: theme.textTheme.bodyMedium,
        ),
        const SizedBox(height: 14),
        if (_inventoryError != null)
          ElysiumCard(
            accent: ElysiumColors.caution,
            child: Row(
              children: [
                const Icon(Icons.info_outline, color: ElysiumColors.caution),
                const SizedBox(width: 12),
                Expanded(
                  child: Text(_inventoryError!, style: theme.textTheme.bodyLarge),
                ),
              ],
            ),
          ),
        if (visible.isEmpty && _inventoryError == null) _emptyDevicesCard(),
        for (final device in visible) ...[
          _deviceCard(device),
          const SizedBox(height: 12),
        ],
        if (_pendingDeviceId != null)
          ElysiumCard(
            accent: ElysiumColors.caution,
            child: Row(
              children: [
                const SizedBox(
                  width: 18,
                  height: 18,
                  child: CircularProgressIndicator(strokeWidth: 2),
                ),
                const SizedBox(width: 14),
                Expanded(
                  child: Text(
                    'Povel sa spracúva. Stav zariadenia ešte nie je potvrdený.',
                    style: theme.textTheme.bodyLarge,
                  ),
                ),
              ],
            ),
          ),
        if (_lastCommand != null) ...[
          const SizedBox(height: 12),
          _commandCard(_lastCommand!),
        ],
        if (_commandMessage != null) ...[
          const SizedBox(height: 12),
          ElysiumCard(
            accent: ElysiumColors.caution,
            child: Text(_commandMessage!, style: theme.textTheme.bodyLarge),
          ),
        ],
      ],
    );
  }

  /// Prečo je zoznam prázdny.
  ///
  /// Prázdno má tri rôzne príčiny a nesmú vyzerať rovnako: ešte sme sa nepozreli,
  /// pozreli sme sa a naozaj tam nič nie je, alebo sa pozrieť nedá. Posledné dve
  /// vyzerajú v odpovedi identicky, preto inventár nesie so sebou stav prepojenia.
  Widget _emptyDevicesCard() {
    final theme = Theme.of(context);
    final inventory = _inventory;
    if (inventory == null) {
      return ElysiumCard(
        child: Text(
          'Zatiaľ nie sú načítané žiadne zariadenia.',
          style: theme.textTheme.bodyMedium,
        ),
      );
    }
    if (!inventory.isConnected) {
      return ElysiumCard(
        accent: ElysiumColors.caution,
        child: Text(
          'Prepojenie na Home Assistant nie je aktívne, takže inventár nevidíme. '
          'Prázdny zoznam tu neznamená prázdnu domácnosť.',
          style: theme.textTheme.bodyLarge,
        ),
      );
    }
    if (inventory.devices.isEmpty) {
      return ElysiumCard(
        child: Text(
          'Home Assistant nehlási žiadne svetlo ani zásuvku. Domácnosť je '
          'z pohľadu Genesisu prázdna.',
          style: theme.textTheme.bodyMedium,
        ),
      );
    }
    return ElysiumCard(
      child: Text(
        'V tejto miestnosti nie je žiadne zariadenie.',
        style: theme.textTheme.bodyMedium,
      ),
    );
  }

  /// Stav jednotky a stav prepojenia na Home Assistant, oddelene.
  ///
  /// `/health` odpovedá na jednu otázku — či beží HTTP server jednotky — a
  /// odpovedá na ňu zeleno aj vtedy, keď WebSocket sedenie k Home Assistantovi
  /// spadlo. Keby panel ukazoval len ju, kontrolka by svietila nad inventárom,
  /// ktorý sa už nehýbe.
  Widget _diagnosticsSection() {
    final theme = Theme.of(context);
    final colors = ElysiumColors.of(context);
    final link = _diagnostics?.homeAssistant;
    final (linkLabel, linkColor) = _linkPresentation(link);
    final (healthLabel, healthColor) = _healthPresentation();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const ElysiumSectionLabel('Diagnostika'),
        const SizedBox(height: 10),
        Text(
          'Odpovedajúca jednotka a dostupný Home Assistant sú dve rôzne veci. '
          'Zelená jednotka neznamená, že sa inventár hýbe.',
          style: theme.textTheme.bodyMedium,
        ),
        const SizedBox(height: 14),
        ElysiumCard(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _diagnosticsRow(
                'Genesis jednotka',
                'Odpoveď na /health',
                healthLabel,
                healthColor,
              ),
              const SizedBox(height: 14),
              _diagnosticsRow(
                'Home Assistant',
                'WebSocket sedenie a inventár',
                linkLabel,
                linkColor,
              ),
              if (_diagnosticsError != null) ...[
                const SizedBox(height: 14),
                Text(_diagnosticsError!, style: theme.textTheme.bodySmall),
              ],
              if (link != null) ...[
                const SizedBox(height: 14),
                Divider(color: colors.border, height: 1),
                const SizedBox(height: 14),
                for (final line in _linkDetails(link, _diagnostics!))
                  Padding(
                    padding: const EdgeInsets.only(bottom: 6),
                    child: Text(line, style: theme.textTheme.labelMedium),
                  ),
              ],
            ],
          ),
        ),
      ],
    );
  }

  Widget _diagnosticsRow(
    String title,
    String detail,
    String label,
    Color color,
  ) {
    final theme = Theme.of(context);
    return Row(
      children: [
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(title, style: theme.textTheme.titleMedium),
              const SizedBox(height: 4),
              Text(detail, style: theme.textTheme.labelMedium),
            ],
          ),
        ),
        const SizedBox(width: 12),
        ElysiumStatusPill(label: label, color: color),
      ],
    );
  }

  /// Vydané identity domácnosti. Vidí ich iba vlastník, takže sa načítajú až
  /// keď je rola známa.
  Future<void> _refreshCredentials() async {
    final base = _baseUrl();
    if (base == null || _accessToken.text.isEmpty) return;
    try {
      final api = GenesisApi(baseUrl: base, client: _client);
      final credentials = await api.credentials(_accessToken.text);
      if (mounted) {
        setState(() {
          _credentials = credentials;
          _accessError = null;
        });
      }
    } on GenesisUnauthorized {
      await _accessWasRefused();
    } catch (_) {
      if (mounted) {
        setState(() {
          _credentials = [];
          _accessError = 'Zoznam vydaných identít sa nepodarilo prečítať.';
        });
      }
    }
  }

  /// Vlastník vydá párovací kód.
  Future<void> _issuePairing() async {
    final base = _baseUrl();
    final actor = _pairingActor.text.trim();
    final household = _principal?.householdId;
    if (base == null || household == null || actor.isEmpty) {
      setState(() => _accessError =
          'Na vydanie kódu treba spojenie, rolu vlastníka a identifikátor člena.');
      return;
    }
    setState(() {
      _busyWithAccess = true;
      _accessError = null;
    });
    try {
      final api = GenesisApi(baseUrl: base, client: _client);
      final pairing = await api.createPairing(
        ownerToken: _accessToken.text,
        householdId: household,
        role: _pairingRole,
        actorId: actor,
      );
      if (mounted) {
        setState(() {
          _issuedCode = pairing;
          _pairingActor.clear();
        });
      }
      await _refreshCredentials();
    } on GenesisUnauthorized {
      await _accessWasRefused();
    } catch (_) {
      if (mounted) {
        setState(() => _accessError =
            'Kód sa nepodarilo vydať. Identifikátor člena už môže mať rozbehnuté párovanie.');
      }
    } finally {
      if (mounted) setState(() => _busyWithAccess = false);
    }
  }

  /// Člen uplatní kód a panel si vydaný token uloží.
  ///
  /// Kód ide v tele požiadavky, nie v adrese, a token sa nikde nevypisuje —
  /// z panela sa dostane len do úložiska a do hlavičky `Authorization`.
  Future<void> _redeem() async {
    final base = _baseUrl();
    final code = _redeemCode.text.trim();
    final household = _redeemHousehold.text.trim();
    if (base == null || code.isEmpty || household.isEmpty) {
      setState(() => _accessError =
          'Na uplatnenie treba adresu jednotky, domácnosť a párovací kód.');
      return;
    }
    setState(() {
      _busyWithAccess = true;
      _accessError = null;
    });
    try {
      final api = GenesisApi(baseUrl: base, client: _client);
      final issued = await api.redeemPairing(householdId: household, code: code);
      await _tokenStore.write(issued.token);
      if (mounted) {
        setState(() {
          _accessToken.text = issued.token;
          _redeemCode.clear();
          _accessNotice = null;
          _accessError = null;
        });
      }
      await _refreshDevices();
    } catch (_) {
      // Jednotka nerozlišuje, prečo kód neplatí, a panel to nemá dopĺňať:
      // vypršaný, už uplatnený aj odobraný kód vyzerajú rovnako a hádať, ktorý
      // z nich to bol, by bola informácia o cudzej domácnosti.
      if (mounted) {
        setState(() => _accessError =
            'Kód neplatí. Mohol vypršať, už byť uplatnený alebo odobraný — '
            'požiadaj vlastníka o nový.');
      }
    } finally {
      if (mounted) setState(() => _busyWithAccess = false);
    }
  }

  Future<void> _revoke(GenesisCredential credential) async {
    final base = _baseUrl();
    if (base == null) return;
    setState(() {
      _busyWithAccess = true;
      _accessError = null;
    });
    try {
      final api = GenesisApi(baseUrl: base, client: _client);
      await api.revokeCredential(
        ownerToken: _accessToken.text,
        credentialId: credential.credentialId,
      );
      await _refreshCredentials();
    } on GenesisUnauthorized {
      await _accessWasRefused();
    } catch (_) {
      if (mounted) {
        setState(() => _accessError = 'Prístup sa nepodarilo odobrať.');
      }
    } finally {
      if (mounted) setState(() => _busyWithAccess = false);
    }
  }

  /// Vydávanie a odoberanie prístupu.
  Widget _accessSection() {
    final theme = Theme.of(context);
    final owner = _principal?.role == 'owner';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const ElysiumSectionLabel('Prístup'),
        const SizedBox(height: 10),
        Text(
          'Prístup sa vydáva párovacím kódom a dá sa odobrať. Kód aj token uvidíš '
          'práve raz — jednotka si z nich drží len odtlačok, takže ani vlastník '
          'ich nevie zobraziť druhýkrát.',
          style: theme.textTheme.bodyMedium,
        ),
        const SizedBox(height: 14),
        if (_accessNotice != null) ...[
          ElysiumCard(
            accent: ElysiumColors.danger,
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Icon(Icons.lock_outline, color: ElysiumColors.danger),
                const SizedBox(width: 12),
                Expanded(
                  child: Text(_accessNotice!, style: theme.textTheme.bodyLarge),
                ),
              ],
            ),
          ),
          const SizedBox(height: 12),
        ],
        if (_accessError != null) ...[
          ElysiumCard(
            accent: ElysiumColors.caution,
            child: Text(_accessError!, style: theme.textTheme.bodyLarge),
          ),
          const SizedBox(height: 12),
        ],
        if (_issuedCode != null) ...[
          _issuedCodeCard(_issuedCode!),
          const SizedBox(height: 12),
        ],
        if (owner) ...[
          _issueCard(),
          const SizedBox(height: 12),
          _credentialsCard(),
          const SizedBox(height: 12),
        ] else if (_principal != null) ...[
          ElysiumCard(
            child: Text(
              'Vydávať a odoberať prístup môže iba vlastník domácnosti. '
              'Tvoja rola je ${_principal!.role}.',
              style: theme.textTheme.bodyMedium,
            ),
          ),
          const SizedBox(height: 12),
        ],
        _redeemCard(),
      ],
    );
  }

  /// Kód sa zobrazí raz a panel to hovorí nahlas.
  Widget _issuedCodeCard(GenesisPairing pairing) {
    final theme = Theme.of(context);
    final colors = ElysiumColors.of(context);
    return ElysiumCard(
      accent: colors.gold,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('Párovací kód pre ${pairing.actorId}',
              style: theme.textTheme.titleMedium),
          const SizedBox(height: 10),
          SelectableText(
            pairing.code,
            style: theme.textTheme.headlineSmall?.copyWith(
              fontFeatures: const [],
              letterSpacing: 2,
            ),
          ),
          const SizedBox(height: 10),
          Text(
            'Rola ${pairing.role}. Zobrazuje sa raz — odpíš ho teraz, jednotka ho '
            'druhýkrát nevydá.',
            style: theme.textTheme.bodyMedium,
          ),
          if (pairing.expiresAt != null) ...[
            const SizedBox(height: 6),
            Text(
              'Platí do ${_formatMoment(pairing.expiresAt!.toLocal())}',
              style: theme.textTheme.labelMedium,
            ),
          ],
          const SizedBox(height: 14),
          Align(
            alignment: Alignment.centerLeft,
            child: OutlinedButton(
              onPressed: () => setState(() => _issuedCode = null),
              child: const Text('Mám ho'),
            ),
          ),
        ],
      ),
    );
  }

  Widget _issueCard() {
    final theme = Theme.of(context);
    return ElysiumCard(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('Vydať párovací kód', style: theme.textTheme.titleLarge),
          const SizedBox(height: 16),
          TextField(
            controller: _pairingActor,
            style: theme.textTheme.bodyLarge,
            decoration: const InputDecoration(labelText: 'Identifikátor člena'),
          ),
          const SizedBox(height: 12),
          Wrap(
            spacing: 8,
            children: [
              for (final role in const ['member', 'guest'])
                ChoiceChip(
                  label: Text(role),
                  selected: _pairingRole == role,
                  onSelected: (_) => setState(() => _pairingRole = role),
                ),
            ],
          ),
          const SizedBox(height: 16),
          FilledButton.icon(
            onPressed: _busyWithAccess ? null : _issuePairing,
            icon: const Icon(Icons.key_outlined, size: 18),
            label: const Text('Vydať kód'),
          ),
        ],
      ),
    );
  }

  Widget _credentialsCard() {
    final theme = Theme.of(context);
    final colors = ElysiumColors.of(context);
    final live = _credentials.where((item) => !item.isRevoked).toList();
    final revoked = _credentials.where((item) => item.isRevoked).toList();
    return ElysiumCard(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text('Vydané identity', style: theme.textTheme.titleLarge),
          const SizedBox(height: 10),
          if (_credentials.isEmpty)
            Text(
              'Zatiaľ nie je vydaná žiadna identita. Tokeny z konfigurácie '
              'jednotky sa tu nezobrazujú — nikto ich nevydal, sú bootstrapom.',
              style: theme.textTheme.bodyMedium,
            ),
          for (final credential in [...live, ...revoked]) ...[
            Divider(color: colors.border, height: 24),
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(credential.actorId, style: theme.textTheme.titleMedium),
                      const SizedBox(height: 6),
                      Text(
                        'Rola ${credential.role}',
                        style: theme.textTheme.labelMedium,
                      ),
                      if (credential.issuedAt != null)
                        Text(
                          'Vydané ${_formatMoment(credential.issuedAt!.toLocal())}',
                          style: theme.textTheme.labelMedium,
                        ),
                      Text(
                        credential.isUnused
                            ? 'Zatiaľ nepoužité'
                            : 'Naposledy použité ${_formatMoment(credential.lastUsedAt!.toLocal())}',
                        style: theme.textTheme.labelMedium,
                      ),
                      if (credential.isRevoked)
                        Text(
                          'Odobrané ${_formatMoment(credential.revokedAt!.toLocal())}'
                          '${credential.revokedBy == null ? '' : ' — ${credential.revokedBy}'}',
                          style: theme.textTheme.labelMedium,
                        ),
                    ],
                  ),
                ),
                const SizedBox(width: 12),
                if (credential.isRevoked)
                  const ElysiumStatusPill(
                    label: 'Odobrané',
                    color: ElysiumColors.danger,
                  )
                else
                  OutlinedButton(
                    onPressed: _busyWithAccess ? null : () => _revoke(credential),
                    child: const Text('Odobrať'),
                  ),
              ],
            ),
          ],
        ],
      ),
    );
  }

  Widget _redeemCard() {
    final theme = Theme.of(context);
    return ElysiumCard(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('Mám párovací kód', style: theme.textTheme.titleLarge),
          const SizedBox(height: 10),
          Text(
            'Kód sa posiela v tele požiadavky, nie v adrese. Vydaný token zostane '
            'v úložisku tohto zariadenia a panel ho nikde nevypisuje.',
            style: theme.textTheme.bodyMedium,
          ),
          const SizedBox(height: 16),
          TextField(
            controller: _redeemHousehold,
            style: theme.textTheme.bodyLarge,
            decoration: const InputDecoration(labelText: 'Domácnosť'),
          ),
          const SizedBox(height: 12),
          TextField(
            controller: _redeemCode,
            obscureText: true,
            style: theme.textTheme.bodyLarge,
            decoration: const InputDecoration(labelText: 'Párovací kód'),
            onSubmitted: (_) => _redeem(),
          ),
          const SizedBox(height: 16),
          FilledButton.icon(
            onPressed: _busyWithAccess ? null : _redeem,
            icon: const Icon(Icons.login, size: 18),
            label: const Text('Uplatniť kód'),
          ),
        ],
      ),
    );
  }

  /// Posledné povely z ledgeru.
  ///
  /// Detail jedného povelu sa dá prečítať len vtedy, keď niekto jeho
  /// identifikátor má. Po obnovení stránky ho nemá nikto, takže bez tohto
  /// zoznamu by neistý povel zostal ležať bez toho, aby sa o ňom niekto dozvedel.
  Widget _ledgerSection() {
    final theme = Theme.of(context);
    final uncertain = _commands.where((command) => command.isUncertain).length;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const ElysiumSectionLabel('Ledger'),
        const SizedBox(height: 10),
        Text(
          'Posledné povely domácnosti, najnovší prvý. Ledger je záznam, nie ovládanie — '
          'povel sa z tohto zoznamu nedá zopakovať.',
          style: theme.textTheme.bodyMedium,
        ),
        const SizedBox(height: 14),
        if (_ledgerError != null)
          ElysiumCard(
            accent: ElysiumColors.caution,
            child: Text(_ledgerError!, style: theme.textTheme.bodyLarge),
          )
        else if (_commands.isEmpty)
          ElysiumCard(
            child: Text(
              'Zatiaľ nie sú načítané žiadne povely.',
              style: theme.textTheme.bodyMedium,
            ),
          )
        else ...[
          if (uncertain > 0)
            Padding(
              padding: const EdgeInsets.only(bottom: 12),
              child: ElysiumCard(
                accent: ElysiumColors.caution,
                child: Text(
                  uncertain == 1
                      ? 'Jeden z posledných povelov skončil neisto.'
                      : 'Neisto skončilo $uncertain z posledných povelov.',
                  style: theme.textTheme.bodyLarge,
                ),
              ),
            ),
          for (final command in _commands) ...[
            _ledgerRow(command),
            const SizedBox(height: 10),
          ],
        ],
      ],
    );
  }

  Widget _ledgerRow(GenesisCommand command) {
    final theme = Theme.of(context);
    final (label, _, color, icon) = _commandPresentation(command);
    return ElysiumCard(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, color: color, size: 18),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(command.deviceId, style: theme.textTheme.bodyLarge),
                const SizedBox(height: 6),
                Text(
                  command.statusChangedAt == null
                      ? label
                      : '$label · ${_formatMoment(command.statusChangedAt!.toLocal())}',
                  style: theme.textTheme.labelMedium,
                ),
                const SizedBox(height: 4),
                Text(
                  'Referencia ${command.correlationId}',
                  style: theme.textTheme.labelMedium,
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _connectionSection() {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const ElysiumSectionLabel('Spojenie'),
        const SizedBox(height: 10),
        ElysiumCard(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text('Spojenie s Genesis API', style: theme.textTheme.titleLarge),
              const SizedBox(height: 16),
              TextField(
                controller: _url,
                keyboardType: TextInputType.url,
                style: theme.textTheme.bodyLarge,
                decoration: const InputDecoration(labelText: 'Adresa Genesis API'),
                onSubmitted: (_) => _checkHealth(),
              ),
              const SizedBox(height: 12),
              TextField(
                controller: _accessToken,
                obscureText: true,
                style: theme.textTheme.bodyLarge,
                decoration: const InputDecoration(labelText: 'Prístupový token'),
                onChanged: (_) => setState(() {
                  _principal = null;
                  _inventory = null;
                }),
              ),
              const SizedBox(height: 16),
              Wrap(spacing: 12, runSpacing: 12, children: [
                FilledButton.icon(
                  onPressed: _checkHealth,
                  icon: const Icon(Icons.refresh, size: 18),
                  label: const Text('Skontrolovať spojenie'),
                ),
                OutlinedButton.icon(
                  onPressed: _refreshDevices,
                  icon: const Icon(Icons.devices, size: 18),
                  label: const Text('Načítať zariadenia'),
                ),
              ]),
            ],
          ),
        ),
      ],
    );
  }

  Widget _deviceCard(GenesisDevice device) {
    final theme = Theme.of(context);
    final colors = ElysiumColors.of(context);
    final stale = device.isStale;
    final on = device.power == true;
    final uncertain = _uncertainCommandFor(device.id);
    final state = stale
        ? 'Stav zastaraný alebo neznámy'
        : on
            ? 'Zapnuté'
            : 'Vypnuté';
    final stateColor = stale
        ? ElysiumColors.caution
        : on
            ? ElysiumColors.teal
            : colors.textTertiary;
    final disabled = stale ||
        !device.writable ||
        _principal?.canControlDevices != true ||
        _pendingDeviceId != null ||
        uncertain != null;
    return ElysiumCard(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Container(
                width: ElysiumLayout.minimumTapTarget,
                height: ElysiumLayout.minimumTapTarget,
                decoration: BoxDecoration(
                  color: on && !stale ? colors.goldWash(38) : colors.surfaceElevated,
                  shape: BoxShape.circle,
                ),
                child: Icon(
                  on && !stale ? Icons.lightbulb : Icons.lightbulb_outline,
                  color: on && !stale ? colors.gold : colors.textTertiary,
                ),
              ),
              const SizedBox(width: 16),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(device.name, style: theme.textTheme.titleMedium),
                    const SizedBox(height: 8),
                    ElysiumStatusPill(label: state, color: stateColor),
                    if (device.observedAt != null) ...[
                      const SizedBox(height: 8),
                      Text(
                        'Posledná zmena v HA: ${_formatMoment(device.observedAt!.toLocal())}',
                        style: theme.textTheme.labelMedium,
                      ),
                    ],
                  ],
                ),
              ),
              const SizedBox(width: 12),
              Switch(
                value: device.power ?? false,
                onChanged: disabled ? null : (_) => _setPower(device),
              ),
            ],
          ),
          if (uncertain != null) ...[
            const SizedBox(height: 14),
            Text(
              'Posledný povel na toto zariadenie skončil neisto, takže prepínač je zamknutý. '
              'Referencia ${uncertain.correlationId}.',
              style: theme.textTheme.bodySmall,
            ),
          ],
        ],
      ),
    );
  }

  /// Ako sa jeden stav povelu hovorí nahlas.
  ///
  /// Šesť stavov ledgeru sa rozlišuje po jednom a stav, ktorý panel nepozná, sa
  /// nikdy nezobrazí ako úspech — keby jednotka niekedy vydala siedmy, mlčky
  /// prijatý „prijaté" by bola nepravda o niečom, čo sa už mohlo stať.
  (String, String, Color, IconData) _commandPresentation(GenesisCommand command) {
    final colors = ElysiumColors.of(context);
    return switch (command.status) {
      'accepted' => (
          'Prijaté',
          'Genesis povel zapísal a ešte ho neodoslal.',
          colors.gold,
          Icons.schedule_outlined,
        ),
      'sent' => (
          'Odoslané',
          'Povel je u Home Assistanta; potvrdenie ešte neprišlo.',
          colors.gold,
          Icons.send_outlined,
        ),
      'provider_confirmed' => (
          'Prijal Home Assistant',
          'Home Assistant povel potvrdil. O samotnom zariadení to nehovorí nič.',
          ElysiumColors.caution,
          Icons.cloud_done_outlined,
        ),
      'device_confirmed' => (
          'Potvrdilo zariadenie',
          'Zariadenie zmenu ohlásilo.',
          ElysiumColors.teal,
          Icons.check_circle_outline,
        ),
      'unknown' => (
          'Neistý výsledok',
          'Nikto nevie, či sa zmena stala. Zopakovanie sa preto neponúka.',
          ElysiumColors.caution,
          Icons.help_outline,
        ),
      'failed' => (
          'Zlyhalo',
          'Povel sa nevykonal.',
          ElysiumColors.danger,
          Icons.error_outline,
        ),
      _ => (
          'Stav, ktorý panel nepozná',
          'Tento stav panel nevie vyhodnotiť, takže ho nepočíta za úspech.',
          ElysiumColors.caution,
          Icons.help_outline,
        ),
    };
  }

  Widget _commandCard(GenesisCommand command) {
    final theme = Theme.of(context);
    final (label, detail, accent, icon) = _commandPresentation(command);
    return ElysiumCard(
      accent: accent,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Icon(icon, color: accent),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(label, style: theme.textTheme.titleMedium),
                    const SizedBox(height: 4),
                    Text(detail, style: theme.textTheme.bodyLarge),
                  ],
                ),
              ),
            ],
          ),
          const SizedBox(height: 14),
          for (final line in _commandDetails(command))
            Padding(
              padding: const EdgeInsets.only(bottom: 6),
              child: Text(line, style: theme.textTheme.labelMedium),
            ),
          const SizedBox(height: 10),
          Align(
            alignment: Alignment.centerLeft,
            child: OutlinedButton.icon(
              onPressed: _readingCommand ? null : _readCommandAgain,
              icon: const Icon(Icons.sync, size: 18),
              label: const Text('Obnoviť stav'),
            ),
          ),
        ],
      ),
    );
  }

  /// Čo presne je o povele známe: čas, dôkaz, dôvod a referencia.
  List<String> _commandDetails(GenesisCommand command) {
    final lines = <String>[];
    if (command.statusChangedAt != null) {
      lines.add('Stav sa zmenil ${_formatMoment(command.statusChangedAt!.toLocal())}');
    }
    final evidence = command.evidence;
    if (evidence != null) {
      final what = switch (evidence.kind) {
        'provider_ack' => 'Potvrdenie Home Assistanta',
        'device_observation' => 'Pozorovanie na zariadení',
        _ => 'Dôkaz',
      };
      final at = evidence.at == null
          ? ''
          : ', ${_formatMoment(evidence.at!.toLocal())}';
      lines.add('$what: ${evidence.reference}$at');
    } else if (command.providerAcknowledged) {
      // Prechod na neistý stav dôkaz zo snapshotu zmaže, ale úroveň potvrdenia
      // zostáva. Bez tejto vety by neistý povel vyzeral, že sa nestalo nič.
      lines.add('V tomto stave bez dôkazu; Home Assistant prijatie potvrdil predtým.');
    } else {
      lines.add('Bez dôkazu.');
    }
    if (command.reason != null) lines.add('Dôvod: ${command.reason}');
    lines.add('Zariadenie ${command.deviceId}');
    lines.add('Referencia pre nahlásenie: ${command.correlationId}');
    lines.add('Povel ${command.commandId}');
    return lines;
  }

  (String, Color) _healthPresentation() => switch (_status) {
        ConnectionStatus.checking => ('Overujem', ElysiumColors.caution),
        ConnectionStatus.online => ('Odpovedá', ElysiumColors.teal),
        ConnectionStatus.offline => ('Neodpovedá', ElysiumColors.danger),
      };

  (String, Color) _linkPresentation(GenesisHaLink? link) {
    if (link == null) return ('Neznámy', ElysiumColors.caution);
    return switch (link.state) {
      'connected' => ('Spojené', ElysiumColors.teal),
      'connecting' => ('Spája sa', ElysiumColors.caution),
      'disconnected' => ('Spojenie spadlo', ElysiumColors.danger),
      'not_configured' => ('Nenastavené', ElysiumColors.caution),
      _ => ('Neznámy', ElysiumColors.caution),
    };
  }

  List<String> _linkDetails(GenesisHaLink link, GenesisDiagnostics diagnostics) {
    final lines = <String>[];
    if (link.since != null) {
      lines.add('V tomto stave od ${_formatMoment(link.since!.toLocal())}');
    }
    if (link.lastInventoryAt != null) {
      lines.add(
        'Inventár naposledy načítaný ${_formatMoment(link.lastInventoryAt!.toLocal())}, '
        'zariadení: ${link.lastInventoryDevices}',
      );
    } else {
      lines.add('Inventár sa v tomto behu ešte nenačítal.');
    }
    if (link.isUnconfigured) {
      lines.add('Prepojenie na Home Assistant nie je nastavené. Nie je to porucha.');
    }
    if (link.lastError != null) {
      lines.add('Naposledy skončilo: ${_linkErrorText(link.lastError!)}');
    }
    lines.add('Verzia jednotky ${diagnostics.version}');
    return lines;
  }

  /// Kategória ukončenia sedenia po slovensky. Jednotka vydáva len kategóriu —
  /// ani adresu, ani token — a panel na tom nič nedopĺňa.
  String _linkErrorText(String category) => switch (category) {
        'connection' => 'sedenie sa nepodarilo otvoriť',
        'authentication' => 'Home Assistant token neprijal',
        'protocol' => 'odpoveď nebola tá, ktorú Genesis čaká',
        'disconnected' => 'sedenie spadlo',
        _ => category,
      };

  Widget _navigation(bool wide) {
    final theme = Theme.of(context);
    final inventory = _inventory;
    final withoutArea = inventory?.devicesWithoutArea ?? const <GenesisDevice>[];
    return ListView(
      padding: const EdgeInsets.symmetric(
        horizontal: 16,
        vertical: ElysiumLayout.sectionSpacing,
      ),
      children: [
        if (wide) ...[
          const Padding(
            padding: EdgeInsets.symmetric(horizontal: 8),
            child: ElysiumWordmark('ELYSIUM', size: 15),
          ),
          const SizedBox(height: ElysiumLayout.sectionSpacing),
        ],
        const Padding(
          padding: EdgeInsets.symmetric(horizontal: 8),
          child: ElysiumSectionLabel('Domácnosť'),
        ),
        const SizedBox(height: 8),
        _navTile(
          label: _householdLabel,
          icon: Icons.home_outlined,
          selected: _scope == AreaScope.whole,
          onTap: () => _select(wide, AreaScope.whole, null),
        ),
        const SizedBox(height: ElysiumLayout.sectionSpacing),
        const Padding(
          padding: EdgeInsets.symmetric(horizontal: 8),
          child: ElysiumSectionLabel('Miestnosti'),
        ),
        const SizedBox(height: 8),
        if (inventory == null)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8),
            child: Text(
              'Miestnosti sa načítajú z domácnosti po zadaní adresy a tokenu.',
              style: theme.textTheme.labelMedium,
            ),
          )
        else ...[
          for (final area in inventory.areas)
            _navTile(
              label: area.name,
              icon: Icons.door_front_door_outlined,
              selected: _scope == AreaScope.area && _areaId == area.areaId,
              onTap: () => _select(wide, AreaScope.area, area.areaId),
            ),
          // Vlastná skupina, nie vymyslená miestnosť: zariadenie bez priradenia
          // sa musí dať nájsť, ale nepatrí do žiadnej existujúcej miestnosti.
          if (withoutArea.isNotEmpty)
            _navTile(
              label: 'Bez miestnosti (${withoutArea.length})',
              icon: Icons.help_outline,
              selected: _scope == AreaScope.withoutArea,
              onTap: () => _select(wide, AreaScope.withoutArea, null),
            ),
          if (inventory.areas.isEmpty && withoutArea.isEmpty)
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 8),
              child: Text(
                inventory.isConnected
                    ? 'Home Assistant nehlási žiadne miestnosti.'
                    : 'Prepojenie na Home Assistant nie je aktívne, miestnosti nevidíme.',
                style: theme.textTheme.labelMedium,
              ),
            ),
          if (inventory.roomsIncomplete) ...[
            const SizedBox(height: 12),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 8),
              child: Text(
                'Register miestností sa nepodarilo prečítať celý, takže miestnosti '
                'tu chýbajú aj vtedy, keď ich domácnosť má. Zariadenia sú tu všetky '
                'a ovládať sa dajú. Register vyžaduje administrátorský token '
                'Home Assistanta.',
                style: theme.textTheme.labelMedium,
              ),
            ),
          ],
        ],
      ],
    );
  }

  void _select(bool wide, AreaScope scope, String? areaId) {
    setState(() {
      _scope = scope;
      _areaId = areaId;
    });
    if (!wide) Navigator.pop(context);
  }

  Widget _navTile({
    required String label,
    required IconData icon,
    required bool selected,
    required VoidCallback onTap,
  }) {
    final theme = Theme.of(context);
    final colors = ElysiumColors.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Material(
        color: selected ? colors.goldWash(28) : Colors.transparent,
        borderRadius: BorderRadius.circular(ElysiumLayout.controlCornerRadius),
        child: InkWell(
          onTap: onTap,
          borderRadius: BorderRadius.circular(ElysiumLayout.controlCornerRadius),
          child: Container(
            constraints: const BoxConstraints(minHeight: ElysiumLayout.minimumTapTarget),
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
            child: Row(
              children: [
                Icon(icon, color: selected ? colors.gold : colors.textTertiary, size: 18),
                const SizedBox(width: 12),
                Expanded(
                  child: Text(
                    label,
                    style: selected
                        ? theme.textTheme.labelLarge?.copyWith(color: colors.gold)
                        : theme.textTheme.bodyLarge,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  Widget _statusChip() {
    final (label, color) = switch (_status) {
      ConnectionStatus.checking => ('Overujem', ElysiumColors.caution),
      ConnectionStatus.online => ('Online', ElysiumColors.teal),
      ConnectionStatus.offline => ('Offline', ElysiumColors.danger),
    };
    return ElysiumStatusPill(label: label, color: color);
  }
}

/// `dd.MM.yyyy HH:mm:ss` without pulling in a date-formatting dependency. The
/// raw `DateTime.toString()` the panel printed before shows microseconds, which
/// reads like a log line rather than a timestamp.
String _formatMoment(DateTime moment) {
  String two(int value) => value.toString().padLeft(2, '0');
  return '${two(moment.day)}.${two(moment.month)}.${moment.year} '
      '${two(moment.hour)}:${two(moment.minute)}:${two(moment.second)}';
}
