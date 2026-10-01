import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/foundation.dart';
import 'package:http/http.dart' as http;

import 'genesis_api.dart';
import 'theme.dart';
import 'token_store.dart';

void main() => runApp(const GenesisApp());

/// Adresa Genesis API, s ktorou sa panel otvorí.
///
/// Vo webe je to pôvod otvorenej stránky: panel sa podáva cez Home Assistant
/// Ingress, takže API je na tej istej adrese a hádať netreba nič.
///
/// V nainštalovanej aplikácii je to **prázdne**, kým to niekto nenastaví cez
/// `--dart-define=GENESIS_API_URL=...`. Predtým tu bolo `http://localhost:8765`,
/// čo je na iPade sám iPad — testovacia adresa zabudnutá v produkčnom builde.
/// Aplikácia naozaj nevie, kde jednotka je, a jediná poctivá odpoveď je spýtať
/// sa; vymyslená adresa by len vyrobila spojenie, ktoré nikdy nenastane.
String genesisDefaultApiUrl() {
  const configured = String.fromEnvironment('GENESIS_API_URL');
  if (configured.isNotEmpty) return configured;
  return kIsWeb ? Uri.base.resolve('.').toString() : '';
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
  List<GenesisGrant> _grants = [];
  String? _grantsError;
  final _transcript = TextEditingController();

  /// Súhlas s uložením prepisu. **Vypnutý, kým ho človek nezapne** — prepis je
  /// obsah toho, čo niekto povedal vo svojej domácnosti.
  bool _storeTranscript = false;
  GenesisVoiceOutcome? _spokenOutcome;
  String? _voiceError;
  bool _speaking = false;
  List<GenesisVoiceAuditEvent> _voiceAudit = [];
  String? _voiceAuditError;
  List<GenesisBackup> _backups = [];
  String? _backupsError;

  /// Nastavené zálohovanie je iné než zálohovanie, ktoré nikto nepoužil.
  bool _backupsConfigured = true;
  String? _backupNotice;
  bool _backingUp = false;

  /// Grant, nad ktorým práve beží vyžiadané zosúladenie. Druhé stlačenie by
  /// nespôsobilo druhý povel — jednotka to nepustí — ale tlačidlo, ktoré
  /// nereaguje, vyzerá rozbito.
  String? _reconciling;
  String? _reconcileNotice;

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
      _grants = [];
      _grantsError = null;
      _reconcileNotice = null;
      _spokenOutcome = null;
      _voiceAudit = [];
      _voiceAuditError = null;
      _voiceError = null;
      _backups = [];
      _backupsError = null;
      _backupNotice = null;
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
      if (principal.role == 'owner') {
        await _refreshCredentials();
        await _refreshBackups();
      }
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
    await _refreshAccess(api);
    await _refreshVoiceAudit(api);
  }

  /// Audit hlasových rozhodnutí. Číta ho každá rola — kto v domácnosti žije, má
  /// vedieť, čo tu hlas urobil, aj keď to nebol on.
  Future<void> _refreshVoiceAudit(GenesisApi api) async {
    try {
      final trail = await api.voiceAudit(_accessToken.text);
      if (mounted) {
        setState(() {
          _voiceAudit = trail;
          _voiceAuditError = null;
        });
      }
    } on GenesisUnauthorized {
      await _accessWasRefused();
    } catch (_) {
      if (mounted) {
        setState(() {
          _voiceAudit = [];
          _voiceAuditError =
              'Audit hlasu sa nepodarilo prečítať. Nevieme, čo sa rozhodlo.';
        });
      }
    }
  }

  /// Časové prístupy. Číta to každá rola — kto v domácnosti žije, má vedieť, že
  /// sa mu niečo zamyká samo.
  Future<void> _refreshAccess(GenesisApi api) async {
    try {
      final grants = await api.access(_accessToken.text);
      if (mounted) {
        setState(() {
          _grants = grants;
          _grantsError = null;
        });
      }
    } on GenesisUnauthorized {
      await _accessWasRefused();
    } catch (_) {
      // Prázdny zoznam by tvrdil, že žiadny časový prístup nie je otvorený.
      // Nevieme to — a práve tu by to bola tá najhoršia nepravda.
      if (mounted) {
        setState(() {
          _grants = [];
          _grantsError =
              'Časové prístupy sa nepodarilo prečítať. Nevieme, či je niektorý otvorený.';
        });
      }
    }
  }

  /// Vyžiada zosúladenie jedného grantu.
  ///
  /// Jednotka týmto nedokáže nič otvoriť ani predĺžiť a platné okno neskráti.
  /// Stav sa prekreslí z odpovede, nie z domnienky o tom, čo stlačenie spôsobilo.
  Future<void> _reconcile(GenesisGrant grant) async {
    final base = _baseUrl();
    if (base == null || _accessToken.text.isEmpty) return;
    setState(() {
      _reconciling = grant.decisionId;
      _reconcileNotice = null;
    });
    try {
      final api = GenesisApi(baseUrl: base, client: _client);
      final receipt = await api.reconcileAccess(
        ownerToken: _accessToken.text,
        decisionId: grant.decisionId,
      );
      if (!mounted) return;
      setState(() {
        _grants = [
          for (final existing in _grants)
            if (existing.decisionId == receipt.grant.decisionId)
              receipt.grant
            else
              existing,
        ];
        _reconcileNotice = _reconcileOutcomeText(receipt);
      });
    } on GenesisUnauthorized {
      await _accessWasRefused();
    } catch (_) {
      if (mounted) {
        setState(() => _reconcileNotice =
            'Zosúladenie sa nepodarilo vyžiadať. Stav grantu sa nezmenil.');
      }
    } finally {
      if (mounted) setState(() => _reconciling = null);
    }
  }

  /// Čo sa stalo, vetou. `settled` a `attempted` sa nezlievajú: prvé znamená
  /// dokázateľne zatvorený prístup, druhé že sa o to Genesis pokúsil a dôkaz
  /// nemá. Zliať ich by znamenalo tvrdiť o fyzickom svete niečo, čo nikto
  /// nepotvrdil.
  String _reconcileOutcomeText(GenesisReconcileReceipt receipt) =>
      switch (receipt.outcome) {
        'settled' => 'Prístup je zatvorený a potvrdený.',
        'attempted' =>
          'Uzavretie je zapísané, ale zariadenie ho nepotvrdilo. Incident zostáva otvorený.',
        'unchanged' =>
          'Nič sa nezmenilo: zariadenie je nedostupné alebo sa čaká na ďalší pokus.',
        'not_due' =>
          'Okno tohto prístupu ešte platí, takže sa nezatvára. Odobrať prístup skôr nie je zosúladenie.',
        'not_open' => 'Tento prístup už nie je otvorený.',
        _ => 'Jednotka odpovedala stavom, ktorý panel nepozná.',
      };

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
    _transcript.dispose();
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
                  _operationsSection(),
                  const SizedBox(height: ElysiumLayout.sectionSpacing + 8),
                  _grantsSection(),
                  const SizedBox(height: ElysiumLayout.sectionSpacing + 8),
                  _voiceSection(),
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

  /// Odošle prepis jednotke.
  ///
  /// Výsledok sa nastaví pri každom zo štyroch stavov. Nejednoznačný a
  /// zamietnutý povel prichádzajú s 422, čo klient nerieši ako chybu — je to
  /// odpoveď o domácnosti, a práve tam človek najviac potrebuje dôvod.
  Future<void> _speak() async {
    final base = _baseUrl();
    final household = _inventory?.householdId;
    final transcript = _transcript.text.trim();
    if (base == null || household == null || transcript.isEmpty) return;
    setState(() {
      _speaking = true;
      _voiceError = null;
    });
    try {
      final api = GenesisApi(baseUrl: base, client: _client);
      final outcome = await api.speak(
        accessToken: _accessToken.text,
        householdId: household,
        transcript: transcript,
        storeTranscript: _storeTranscript,
      );
      if (!mounted) return;
      setState(() {
        _spokenOutcome = outcome;
        // Pole sa čistí len vtedy, keď sa niečo stalo alebo sa už nedá
        // zopakovať. Pri nepochopenom povele text zostáva, aby sa dal upraviť.
        if (!outcome.isUnclear) _transcript.clear();
      });
      await _refreshVoiceAudit(api);
      if (outcome.wasExecuted) await _refreshDevices();
    } on GenesisUnauthorized {
      await _accessWasRefused();
    } catch (_) {
      if (mounted) {
        setState(() {
          _spokenOutcome = null;
          _voiceError =
              'Povel sa nepodarilo odoslať. Nevieme, či sa niečo vykonalo — '
              'skontrolujte stav zariadenia a audit.';
        });
      }
    } finally {
      if (mounted) setState(() => _speaking = false);
    }
  }

  /// Potvrdí citlivú akciu. Až toto ju vykoná; dovtedy sa nestalo nič.
  Future<void> _confirmSpoken(String confirmationId) async {
    final base = _baseUrl();
    final household = _inventory?.householdId;
    if (base == null || household == null) return;
    setState(() {
      _speaking = true;
      _voiceError = null;
    });
    try {
      final api = GenesisApi(baseUrl: base, client: _client);
      final outcome = await api.confirmSpoken(
        accessToken: _accessToken.text,
        householdId: household,
        confirmationId: confirmationId,
      );
      if (!mounted) return;
      setState(() => _spokenOutcome = outcome);
      await _refreshVoiceAudit(api);
      if (outcome.wasExecuted) await _refreshDevices();
    } on GenesisUnauthorized {
      await _accessWasRefused();
    } catch (_) {
      if (mounted) {
        setState(() => _voiceError =
            'Potvrdenie sa nepodarilo odoslať. Platí krátko, takže ho možno '
            'bude treba vyžiadať znova.');
      }
    } finally {
      if (mounted) setState(() => _speaking = false);
    }
  }

  /// Zálohy. Číta ich iba vlastník, takže sa načítajú až keď je rola známa.
  Future<void> _refreshBackups() async {
    final base = _baseUrl();
    if (base == null || _accessToken.text.isEmpty) return;
    try {
      final api = GenesisApi(baseUrl: base, client: _client);
      final found = await api.backups(_accessToken.text);
      if (mounted) {
        setState(() {
          _backups = found;
          _backupsConfigured = true;
          _backupsError = null;
        });
      }
    } on GenesisUnauthorized {
      await _accessWasRefused();
    } catch (error) {
      if (mounted) {
        setState(() {
          _backups = [];
          // 503 je nenastavené zálohovanie, nie porucha. Zliať to s chybou by
          // znamenalo, že prevádzkovateľ hľadá problém tam, kde žiadny nie je.
          _backupsConfigured = !'$error'.contains('503');
          _backupsError = _backupsConfigured
              ? 'Zálohy sa nepodarilo prečítať. Nevieme, či nejaká existuje.'
              : null;
        });
      }
    }
  }

  Future<void> _createBackup() async {
    final base = _baseUrl();
    if (base == null || _accessToken.text.isEmpty) return;
    setState(() {
      _backingUp = true;
      _backupNotice = null;
    });
    try {
      final api = GenesisApi(baseUrl: base, client: _client);
      final written = await api.createBackup(_accessToken.text);
      if (!mounted) return;
      setState(() => _backupNotice =
          'Záloha je zapísaná: ${written.size}, ${written.path}.');
      await _refreshBackups();
    } on GenesisUnauthorized {
      await _accessWasRefused();
    } on GenesisBackupExists {
      if (mounted) {
        setState(() => _backupNotice =
            'Záloha pre tento okamih už existuje a jednotka ju neprepíše. '
            'Predchádzajúca záloha zostáva neporušená.');
      }
    } catch (_) {
      if (mounted) {
        setState(() => _backupNotice =
            'Zálohu sa nepodarilo vytvoriť. Zoznam nižšie hovorí, čo jednotka má.');
      }
    } finally {
      if (mounted) setState(() => _backingUp = false);
    }
  }

  /// Otvorené incidenty naprieč grantmi.
  ///
  /// Počíta sa z toho, čo panel už má z `GET /v1/access` — nie je na to vlastné
  /// API a ani ho netreba vyrábať.
  int get _openIncidents =>
      _grants.fold(0, (total, grant) => total + grant.openIncidents.length);

  /// Prevádzka: čo beží, čo sa pokazilo a čo je zazálohované.
  ///
  /// Päť vecí a päť riadkov. Zliať ich do jedného „stav systému" by zahodilo
  /// presne tú informáciu, pre ktorú sem prevádzkovateľ chodí: odpovedajúca
  /// jednotka nič nehovorí o Home Assistantovi, ten nič o incidentoch a žiadny z
  /// nich nič o tom, či existuje záloha.
  Widget _operationsSection() {
    final theme = Theme.of(context);
    final owner = _principal?.role == 'owner';
    final link = _diagnostics?.homeAssistant;
    final (healthLabel, healthColor) = _healthPresentation();
    final (linkLabel, linkColor) = _linkPresentation(link);
    final incidents = _openIncidents;
    final newest = _backups.isEmpty ? null : _backups.first;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const ElysiumSectionLabel('Prevádzka'),
        const SizedBox(height: 10),
        Text(
          'Päť vecí, ktoré sa môžu pokaziť nezávisle od seba, a preto majú päť '
          'riadkov. Odpovedajúca jednotka nehovorí nič o Home Assistantovi — '
          '`/health` odpovedá aj vtedy, keď WebSocket sedenie spadlo — ten nič o '
          'incidentoch a žiadny z nich nič o tom, či existuje záloha.',
          style: theme.textTheme.bodyMedium,
        ),
        const SizedBox(height: 14),
        ElysiumCard(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _diagnosticsRow('Genesis jednotka', 'Odpoveď na /health',
                  healthLabel, healthColor),
              const SizedBox(height: 14),
              _diagnosticsRow('Home Assistant', 'WebSocket sedenie',
                  linkLabel, linkColor),
              const SizedBox(height: 14),
              _diagnosticsRow(
                'Inventár',
                _inventory == null
                    ? 'Nenačítaný'
                    : '${_inventory!.devices.length} zariadení, '
                        '${_inventory!.areas.length} miestností',
                _inventoryStatus().$1,
                _inventoryStatus().$2,
              ),
              const SizedBox(height: 14),
              _diagnosticsRow(
                'Otvorené incidenty',
                incidents == 0
                    ? 'Žiadny grant nečaká na zosúladenie'
                    : 'Časový prístup, ktorý sa nepodarilo uzavrieť',
                incidents == 0 ? 'Žiadne' : '$incidents',
                incidents == 0 ? ElysiumColors.teal : ElysiumColors.danger,
              ),
              const SizedBox(height: 14),
              _diagnosticsRow(
                'Posledná záloha',
                _backupDetail(newest),
                _backupStatus(newest).$1,
                _backupStatus(newest).$2,
              ),
              if (_diagnosticsError != null) ...[
                const SizedBox(height: 14),
                Text(_diagnosticsError!, style: theme.textTheme.bodySmall),
              ],
              if (link != null) ...[
                const SizedBox(height: 14),
                Divider(color: ElysiumColors.of(context).border, height: 1),
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
        const SizedBox(height: 12),
        if (owner) _backupCard() else _backupNotForYouCard(),
        const SizedBox(height: 12),
        _restoreCard(),
      ],
    );
  }

  (String, Color) _inventoryStatus() {
    final inventory = _inventory;
    if (inventory == null) return ('Neznámy', ElysiumColors.caution);
    if (!inventory.isConnected) return ('Zastaraný', ElysiumColors.caution);
    if (inventory.roomsIncomplete) {
      return ('Neúplný', ElysiumColors.caution);
    }
    return ('Načítaný', ElysiumColors.teal);
  }

  (String, Color) _backupStatus(GenesisBackup? newest) {
    if (!_backupsConfigured) return ('Nenastavené', ElysiumColors.electricBlue);
    if (_backupsError != null) return ('Neznáme', ElysiumColors.caution);
    if (_principal != null && _principal!.role != 'owner') {
      return ('Len vlastník', ElysiumColors.electricBlue);
    }
    if (newest == null) return ('Žiadna', ElysiumColors.danger);
    return ('Existuje', ElysiumColors.teal);
  }

  String _backupDetail(GenesisBackup? newest) {
    if (!_backupsConfigured) {
      return 'Jednotka nemá nastavený priečinok pre zálohy';
    }
    if (_backupsError != null) return _backupsError!;
    if (_principal != null && _principal!.role != 'owner') {
      return 'Zálohy vidí iba vlastník domácnosti';
    }
    if (newest == null) return 'Žiadna záloha neexistuje';
    return newest.at == null
        ? newest.path
        : '${_formatMoment(newest.at!.toLocal())} · ${newest.size}';
  }

  Widget _backupCard() {
    final theme = Theme.of(context);
    return ElysiumCard(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('Záloha', style: theme.textTheme.titleMedium),
          const SizedBox(height: 4),
          Text(
            'Celý stav Genesis je jeden SQLite súbor. Záloha je `VACUUM INTO` za '
            'behu, takže je celá a platná aj keď sa práve zapisuje — na rozdiel '
            'od skopírovania súboru. Existujúcu zálohu jednotka neprepíše.',
            style: theme.textTheme.labelMedium,
          ),
          const SizedBox(height: 12),
          if (!_backupsConfigured)
            Text(
              'Zálohovanie nie je na jednotke nastavené, takže tlačidlo by '
              'nemalo kam zapisovať.',
              style: theme.textTheme.bodyMedium,
            )
          else
            Align(
              alignment: Alignment.centerLeft,
              child: FilledButton(
                onPressed: _backingUp ? null : _createBackup,
                child: Text(_backingUp ? 'Zapisuje sa…' : 'Vytvoriť zálohu'),
              ),
            ),
          if (_backupNotice != null) ...[
            const SizedBox(height: 10),
            Text(_backupNotice!, style: theme.textTheme.bodySmall),
          ],
          if (_backups.isNotEmpty) ...[
            const SizedBox(height: 12),
            Text('Zálohy na jednotke', style: theme.textTheme.labelMedium),
            for (final item in _backups.take(6))
              Padding(
                padding: const EdgeInsets.only(top: 6),
                child: Text(
                  '${item.at == null ? "—" : _formatMoment(item.at!.toLocal())} · '
                  '${item.size} · ${item.path}',
                  style: theme.textTheme.labelMedium,
                ),
              ),
          ],
        ],
      ),
    );
  }

  Widget _backupNotForYouCard() {
    final theme = Theme.of(context);
    return ElysiumCard(
      child: Text(
        'Zálohu vytvára a vidí iba vlastník domácnosti. Jednotka to odmieta sama '
        '— panel to tu len nenavrhuje.',
        style: theme.textTheme.bodyMedium,
      ),
    );
  }

  /// Prečo tu tlačidlo na obnovu nie je.
  ///
  /// Nie je to chýbajúca funkcia. Služba si nedokáže bezpečne podsunúť súbor
  /// sama sebe pod otvoreným spojením, takže „živá obnova" by bola operácia,
  /// ktorá môže poškodiť presne ten stav, ktorý má zachraňovať. Obnova sa robí
  /// pri zastavenej službe a podľa runbooku.
  Widget _restoreCard() {
    final theme = Theme.of(context);
    return ElysiumCard(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('Obnova a rollback', style: theme.textTheme.titleMedium),
          const SizedBox(height: 4),
          Text(
            'Obnovu panel nerobí a nebude. Nie je to chýbajúca funkcia: služba si '
            'nedokáže bezpečne podsunúť databázu sama sebe pod otvoreným '
            'spojením, takže tlačidlo „obnoviť" by mohlo poškodiť práve ten stav, '
            'ktorý má zachraňovať.',
            style: theme.textTheme.bodyMedium,
          ),
          const SizedBox(height: 10),
          Text(
            'Postup sa robí pri zastavenej službe: zastaviť app, odložiť súbor '
            'ledgeru, nakopírovať zálohu na jeho miesto, spustiť app. Celý '
            'runbook vrátane aktualizácie a rollbacku je v '
            'docs/ELYSIUM-348-obnova.md; staršia verzia novšiu databázu odmietne, '
            'takže rollback nie je len o obraze.',
            style: theme.textTheme.labelMedium,
          ),
        ],
      ),
    );
  }

  /// Hlasový povel textom.
  ///
  /// Mikrofón tu nie je a ani sa nepredstiera: panel neberie zvuk, takže žiadne
  /// surové audio nevzniká a nie je čo ukladať. Rozpoznávanie reči cez Home
  /// Assistant Assist alebo iný výslovný adaptér je samostatný krok — tento
  /// tiket dáva bezpečnú textovú cestu k tomu istému intentu.
  Widget _voiceSection() {
    final theme = Theme.of(context);
    final principal = _principal;
    final canSpeak = principal?.canControlDevices ?? false;
    final ready = _inventory != null && _accessToken.text.isNotEmpty;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const ElysiumSectionLabel('Hlasový povel'),
        const SizedBox(height: 10),
        Text(
          'Prepis prejde tou istou cestou ako hlas: jednotka ho preloží na intent '
          'a ďalej už pracuje so zariadením, nie s textom. Mikrofón v paneli nie '
          'je, takže žiadne surové audio nevzniká — rozpoznávanie reči je '
          'samostatný krok.',
          style: theme.textTheme.bodyMedium,
        ),
        const SizedBox(height: 14),
        if (!ready)
          ElysiumCard(
            child: Text(
              'Bez prístupového tokenu a inventára sa povel odoslať nedá.',
              style: theme.textTheme.bodyMedium,
            ),
          )
        else if (!canSpeak)
          ElysiumCard(
            child: Text(
              'Hlas nedáva viac práv než panel: ovládať zariadenia smie vlastník '
              'a člen, nie hosť. Rozhodnutie robí jednotka, nie panel.',
              style: theme.textTheme.bodyMedium,
            ),
          )
        else
          _speakCard(principal!),
        if (_spokenOutcome != null) ...[
          const SizedBox(height: 12),
          _outcomeCard(_spokenOutcome!),
        ],
        if (_voiceError != null) ...[
          const SizedBox(height: 12),
          ElysiumCard(
            child: Text(_voiceError!, style: theme.textTheme.bodyMedium),
          ),
        ],
        const SizedBox(height: 14),
        _voiceAuditCard(),
      ],
    );
  }

  Widget _speakCard(GenesisPrincipal principal) {
    final theme = Theme.of(context);
    return ElysiumCard(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          TextField(
            controller: _transcript,
            decoration: const InputDecoration(
              labelText: 'Povel',
              hintText: 'napríklad: zhasni svetlo v obývačke',
            ),
            onSubmitted: _speaking ? null : (_) => _speak(),
          ),
          const SizedBox(height: 10),
          // Identitu aktéra určuje jednotka podľa tokenu, nie panel. Píše sa
          // tu preto, že do auditu pôjde práve toto meno.
          Text(
            'Vykoná sa ako ${principal.actorId} (${principal.role}). '
            'Identitu určuje jednotka podľa tokenu.',
            style: theme.textTheme.labelMedium,
          ),
          const SizedBox(height: 10),
          // Zámerne `Checkbox` v riadku, nie `CheckboxListTile`: ten kreslí
          // pozadie a ink na najbližší `Material`, ktorým je tu dekorovaná
          // karta, takže by boli neviditeľné.
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Checkbox(
                value: _storeTranscript,
                onChanged: _speaking
                    ? null
                    : (value) =>
                        setState(() => _storeTranscript = value ?? false),
              ),
              const SizedBox(width: 4),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    const SizedBox(height: 12),
                    Text('Uložiť prepis k povelu',
                        style: theme.textTheme.bodyMedium),
                    const SizedBox(height: 2),
                    Text(
                      'Bez súhlasu si jednotka prepis nenechá — uloží len intent '
                      'a rozhodnutie. Audit funguje aj tak.',
                      style: theme.textTheme.labelMedium,
                    ),
                  ],
                ),
              ),
            ],
          ),
          const SizedBox(height: 10),
          Align(
            alignment: Alignment.centerLeft,
            child: FilledButton(
              onPressed: _speaking ? null : _speak,
              child: Text(_speaking ? 'Odosiela sa…' : 'Odoslať povel'),
            ),
          ),
        ],
      ),
    );
  }

  /// Výsledok povelu. Štyri stavy a každý iná veta — zliať ich do „úspech/chyba"
  /// by zahodilo práve to, čo je na tom bezpečné.
  Widget _outcomeCard(GenesisVoiceOutcome outcome) {
    final theme = Theme.of(context);
    final colors = ElysiumColors.of(context);
    final (label, color) = _outcomePresentation(outcome);
    return ElysiumCard(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(_outcomeTitle(outcome),
                    style: theme.textTheme.titleMedium),
              ),
              const SizedBox(width: 12),
              ElysiumStatusPill(label: label, color: color),
            ],
          ),
          const SizedBox(height: 14),
          Divider(color: colors.border, height: 1),
          const SizedBox(height: 14),
          Text(_outcomeDetail(outcome), style: theme.textTheme.bodyMedium),
          if (outcome.explanation != null) ...[
            const SizedBox(height: 8),
            Text(
              'Dôvod: ${outcome.explanation!.code}',
              style: theme.textTheme.labelMedium,
            ),
          ],
          // Nejednoznačný povel: aspoň sa dá povedať, medzi čím sa jednotka
          // nerozhodla. Hádať jedno z nich by bola tá najhoršia odpoveď.
          if (outcome.candidates.isNotEmpty) ...[
            const SizedBox(height: 10),
            Text('Jednotka sa nerozhodla medzi:',
                style: theme.textTheme.labelMedium),
            for (final candidate in outcome.candidates)
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: Text(_deviceLabel(candidate),
                    style: theme.textTheme.bodyMedium),
              ),
          ],
          if (outcome.wasExecuted && outcome.command != null) ...[
            const SizedBox(height: 10),
            for (final line in _commandDetails(outcome.command!))
              Padding(
                padding: const EdgeInsets.only(bottom: 4),
                child: Text(line, style: theme.textTheme.labelMedium),
              ),
          ],
          if (outcome.needsConfirmation && outcome.confirmationId != null) ...[
            const SizedBox(height: 14),
            Text(
              outcome.expiresAt == null
                  ? 'Potvrdenie platí krátko a práve raz.'
                  : 'Potvrdenie platí do ${_formatMoment(outcome.expiresAt!.toLocal())} a práve raz.',
              style: theme.textTheme.labelMedium,
            ),
            const SizedBox(height: 10),
            Align(
              alignment: Alignment.centerLeft,
              child: FilledButton(
                onPressed: _speaking
                    ? null
                    : () => _confirmSpoken(outcome.confirmationId!),
                child: Text(_speaking ? 'Potvrdzuje sa…' : 'Potvrdiť akciu'),
              ),
            ),
          ],
        ],
      ),
    );
  }

  (String, Color) _outcomePresentation(GenesisVoiceOutcome outcome) =>
      switch (outcome.outcome) {
        'executed' => ('Vykonané', ElysiumColors.teal),
        'confirmation_required' =>
          ('Čaká na potvrdenie', ElysiumColors.caution),
        'unclear' => ('Nevykonané', ElysiumColors.electricBlue),
        'refused' => ('Zamietnuté', ElysiumColors.danger),
        // Stav, ktorý panel nepozná, sa nikdy nezobrazí ako úspech.
        _ => ('Neznámy výsledok', ElysiumColors.caution),
      };

  String _outcomeTitle(GenesisVoiceOutcome outcome) {
    final intent = outcome.intent;
    if (intent == null) return 'Povel sa nevykonal';
    return '${_deviceLabel(intent.deviceId)} — '
        '${intent.value ? "zapnúť" : "vypnúť"}';
  }

  String _outcomeDetail(GenesisVoiceOutcome outcome) => switch (outcome.outcome) {
        'executed' => outcome.explanation?.message ??
            'Povel je v ledgeri. Stav zariadenia je v sekcii Zariadenia.',
        'confirmation_required' =>
          'Citlivá akcia: nič sa nevykonalo a zariadenie sa nepohlo. Jednotka '
              'čaká na výslovné potvrdenie a bez neho neurobí nič.',
        'unclear' => outcome.message ??
            'Jednotka povel nepochopila, takže nevykonala nič.',
        'refused' => outcome.explanation?.message ??
            'Jednotka povel zamietla.',
        _ => 'Jednotka odpovedala stavom, ktorý panel nepozná. '
            'Nepovažujte to za vykonané.',
      };

  /// Názov zariadenia z inventára, inak identifikátor. Vymyslieť názov by
  /// znamenalo tvrdiť o domácnosti niečo, čo z odpovede neplynie.
  String _deviceLabel(String deviceId) {
    final device = _inventory?.devices
        .where((candidate) => candidate.id == deviceId)
        .firstOrNull;
    return device?.name ?? deviceId;
  }

  Widget _voiceAuditCard() {
    final theme = Theme.of(context);
    if (_voiceAuditError != null) {
      return ElysiumCard(
        child: Text(_voiceAuditError!, style: theme.textTheme.bodyMedium),
      );
    }
    if (_voiceAudit.isEmpty) {
      return ElysiumCard(
        child: Text(
          'Audit hlasu je prázdny: jednotka zatiaľ o žiadnom hlasovom povele '
          'nerozhodovala.',
          style: theme.textTheme.bodyMedium,
        ),
      );
    }
    return ElysiumCard(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('Audit hlasu', style: theme.textTheme.titleMedium),
          const SizedBox(height: 4),
          Text(
            'Čo sa rozhodlo, kým a prečo. Prepis tu nie je ani vtedy, keď bol '
            'uložený — audit dokladá rozhodnutie, nie obsah.',
            style: theme.textTheme.labelMedium,
          ),
          const SizedBox(height: 12),
          for (final event in _voiceAudit.take(8))
            Padding(
              padding: const EdgeInsets.only(bottom: 8),
              child: Text(
                '${_auditDecision(event.decision)} · ${event.reason} · '
                '${event.actorId}'
                '${event.at == null ? "" : " · ${_formatMoment(event.at!.toLocal())}"}',
                style: theme.textTheme.labelMedium,
              ),
            ),
        ],
      ),
    );
  }

  String _auditDecision(String decision) => switch (decision) {
        'executed' => 'Vykonané',
        'refused' => 'Zamietnuté',
        'awaiting_confirmation' => 'Čakalo na potvrdenie',
        _ => decision,
      };

  /// Časový prístup: čo Behavior otvorilo a čo sa z toho vrátilo späť.
  ///
  /// Sekcia je postavená na jednom rozlíšení: **logická evidencia na jednotke
  /// nie je stav zariadenia.** Grant môže byť „relocked" a žiarovka svietiť,
  /// alebo „relock_pending" a byť dávno zhasnutá. Preto má každý grant dva
  /// samostatné riadky a panel ich nikdy nezlieva do jedinej vety.
  Widget _grantsSection() {
    final theme = Theme.of(context);
    final now = DateTime.now();
    final open = _grants.where((grant) => grant.isOpen).toList();
    final closed = _grants.where((grant) => !grant.isOpen).toList();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const ElysiumSectionLabel('Časový prístup'),
        const SizedBox(height: 10),
        Text(
          'Prístup, ktorý Behavior otvoril na čas. Evidencia na jednotke a stav '
          'zariadenia sú dve rôzne veci, takže sa tu píšu oddelene: jeden riadok '
          'hovorí, čo si jednotka pamätá, druhý, čo naozaj niekto potvrdil.',
          style: theme.textTheme.bodyMedium,
        ),
        const SizedBox(height: 14),
        if (_grantsError != null)
          ElysiumCard(
            child: Text(_grantsError!, style: theme.textTheme.bodyMedium),
          )
        else if (_accessToken.text.isEmpty)
          ElysiumCard(
            child: Text(
              'Bez prístupového tokenu sa časové prístupy nečítajú.',
              style: theme.textTheme.bodyMedium,
            ),
          )
        else if (_grants.isEmpty)
          ElysiumCard(
            child: Text(
              'Žiadny časový prístup nie je otvorený ani nedovrený.',
              style: theme.textTheme.bodyMedium,
            ),
          )
        else ...[
          for (final grant in [...open, ...closed]) ...[
            _grantCard(grant, now),
            const SizedBox(height: 12),
          ],
          if (_reconcileNotice != null)
            Text(_reconcileNotice!, style: theme.textTheme.bodySmall),
        ],
      ],
    );
  }

  Widget _grantCard(GenesisGrant grant, DateTime now) {
    final theme = Theme.of(context);
    final colors = ElysiumColors.of(context);
    final (stateLabel, stateColor) = _grantPresentation(grant, now);
    final owner = _principal?.role == 'owner';
    final busy = _reconciling == grant.decisionId;
    final actionable = grant.isReconcilable(now);
    return ElysiumCard(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(_grantDeviceLabel(grant), style: theme.textTheme.titleMedium),
                    const SizedBox(height: 4),
                    Text(
                      'Otvorené na ${grant.grantedValue ? "zapnuté" : "vypnuté"}'
                      ' · ${grant.capabilityId}',
                      style: theme.textTheme.labelMedium,
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 12),
              ElysiumStatusPill(label: stateLabel, color: stateColor),
            ],
          ),
          const SizedBox(height: 14),
          Divider(color: colors.border, height: 1),
          const SizedBox(height: 14),
          _grantFact('Evidencia jednotky', _grantLedgerText(grant, now)),
          const SizedBox(height: 10),
          _grantFact('Fyzické potvrdenie', _grantConfirmedText(grant)),
          if (grant.openIncidents.isNotEmpty) ...[
            const SizedBox(height: 14),
            for (final incident in grant.openIncidents)
              Padding(
                padding: const EdgeInsets.only(bottom: 6),
                child: Text(
                  'Otvorený incident (${incident.kind}): ${incident.detail}',
                  style: theme.textTheme.bodySmall
                      ?.copyWith(color: ElysiumColors.caution),
                ),
              ),
            Text(
              'Pokusy o uzavretie: ${grant.closeAttempts}',
              style: theme.textTheme.labelMedium,
            ),
          ],
          const SizedBox(height: 14),
          if (owner)
            Align(
              alignment: Alignment.centerLeft,
              child: OutlinedButton(
                onPressed: actionable && !busy ? () => _reconcile(grant) : null,
                child: Text(busy ? 'Zosúlaďuje sa…' : 'Zosúladiť teraz'),
              ),
            )
          else
            Text(
              'Zosúladenie môže vyžiadať iba vlastník.',
              style: theme.textTheme.labelMedium,
            ),
          if (owner && !actionable) ...[
            const SizedBox(height: 6),
            Text(
              grant.isOpen
                  ? 'Okno ešte platí. Zatvoriť prístup skôr nie je zosúladenie — '
                      'to je odobranie prístupu a má vlastné rozhodnutie.'
                  : 'Tento prístup je uzavretý, takže nie je čo zosúlaďovať.',
              style: theme.textTheme.labelMedium,
            ),
          ],
        ],
      ),
    );
  }

  Widget _grantFact(String title, String detail) {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(title, style: theme.textTheme.labelMedium),
        const SizedBox(height: 2),
        Text(detail, style: theme.textTheme.bodyMedium),
      ],
    );
  }

  /// Názov zariadenia z inventára, keď ho panel má.
  ///
  /// Keď nie, zobrazí sa identifikátor. Vymyslieť názov by znamenalo tvrdiť o
  /// domácnosti niečo, čo z odpovede neplynie — a zariadenie, ktoré z Home
  /// Assistanta zmizlo, je presne ten prípad, kde to človeka zmätie najviac.
  String _grantDeviceLabel(GenesisGrant grant) {
    final device = _inventory?.devices
        .where((candidate) => candidate.id == grant.deviceId)
        .firstOrNull;
    return device?.name ?? grant.deviceId;
  }

  /// Logický stav grantu, slovom. Je to evidencia jednotky, nie stav zariadenia.
  (String, Color) _grantPresentation(GenesisGrant grant, DateTime now) =>
      switch (grant.state) {
        'granted' => ('Bez výsledku', ElysiumColors.caution),
        'active' when grant.expiredAt(now) =>
          ('Po expirácii', ElysiumColors.caution),
        'active' => ('Otvorený', ElysiumColors.teal),
        'relock_pending' => ('Neistý návrat', ElysiumColors.danger),
        'relocked' => ('Vrátený', ElysiumColors.teal),
        'unlock_failed' => ('Nevykonaný', ElysiumColors.electricBlue),
        'superseded' => ('Nahradený', ElysiumColors.electricBlue),
        // Stav, ktorý panel nepozná, sa nikdy nezobrazí ako úspech.
        _ => ('Neznámy stav', ElysiumColors.caution),
      };

  String _grantLedgerText(GenesisGrant grant, DateTime now) {
    final window = grant.expiresAt == null
        ? 'bez známeho konca okna'
        : grant.expiredAt(now)
            ? 'okno uplynulo ${_formatMoment(grant.expiresAt!.toLocal())}'
            : 'platí do ${_formatMoment(grant.expiresAt!.toLocal())}';
    return switch (grant.state) {
      'granted' =>
        'Unlock je prijatý, výsledok sa nezapísal — $window. Po reštarte to '
            'jednotka preberá sama.',
      'active' when grant.unlockConfirmed => 'Prístup je otvorený, $window.',
      'active' =>
        'Prístup je evidovaný ako otvorený, ale unlock nedosiahol vyžadované '
            'potvrdenie ($window).',
      'relock_pending' =>
        'Uzavretie je zapísané a potvrdenie nedošlo, $window.',
      'relocked' => 'Prístup je vrátený späť, $window.',
      'unlock_failed' => 'Unlock sa nevykonal, nie je čo vracať.',
      'superseded' =>
        'To isté zariadenie drží na rovnakej hodnote novšie rozhodnutie.',
      _ => 'Stav „${grant.state}" panel nepozná ($window).',
    };
  }

  /// Čo o fyzickom svete naozaj vieme.
  ///
  /// `provider` a `device` sa nezlievajú: iba druhé hovorí o zariadení. Prvé
  /// znamená, že potvrdil Home Assistant — čo je o jeden krok ďalej od
  /// žiarovky, než sa zdá.
  String _grantConfirmedText(GenesisGrant grant) {
    final confirmed = grant.lastConfirmed;
    if (confirmed == null) {
      return 'Žiadne potvrdenie. Fyzický stav zariadenia nie je známy.';
    }
    final value = switch (confirmed.value) {
      true => 'zapnuté',
      false => 'vypnuté',
      final other => '$other',
    };
    final who = confirmed.isDeviceConfirmed
        ? 'Zariadenie potvrdilo'
        : 'Potvrdil poskytovateľ (nie zariadenie):';
    final at = confirmed.observedAt ?? confirmed.at;
    final when = at == null ? '' : ' · ${_formatMoment(at.toLocal())}';
    final required = confirmed.isDeviceConfirmed ||
            grant.requiredConfirmation != 'device'
        ? ''
        : ' Rozhodnutie vyžadovalo potvrdenie zariadením.';
    return '$who $value$when.$required';
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
