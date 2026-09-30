import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/foundation.dart';
import 'package:http/http.dart' as http;

import 'genesis_api.dart';
import 'theme.dart';

void main() => runApp(const GenesisApp());

String genesisDefaultApiUrl() {
  const configured = String.fromEnvironment('GENESIS_API_URL');
  if (configured.isNotEmpty) return configured;
  return kIsWeb ? Uri.base.resolve('.').toString() : 'http://localhost:8765';
}

class GenesisApp extends StatelessWidget {
  const GenesisApp({super.key, this.client});

  /// Vymeniteľný HTTP klient. Prezentácia neistého výsledku a výpadku Home
  /// Assistanta sa inak nedá otestovať bez skutočnej jednotky, a práve tie dve
  /// veci sa nesmú zobraziť ako úspech.
  final http.Client? client;

  @override
  Widget build(BuildContext context) => MaterialApp(
        title: 'OS Genesis',
        debugShowCheckedModeBanner: false,
        // The app is adaptive and the panel lives inside Home Assistant, so it
        // follows the system rather than forcing one appearance.
        theme: elysiumTheme(Brightness.light),
        darkTheme: elysiumTheme(Brightness.dark),
        themeMode: ThemeMode.system,
        home: GenesisHome(client: client),
      );
}

enum ConnectionStatus { checking, online, offline }

class GenesisHome extends StatefulWidget {
  const GenesisHome({super.key, this.client});

  final http.Client? client;

  @override
  State<GenesisHome> createState() => _GenesisHomeState();
}

class _GenesisHomeState extends State<GenesisHome> {
  final _url = TextEditingController(text: genesisDefaultApiUrl());
  final _accessToken = TextEditingController();
  GenesisPrincipal? _principal;
  late final http.Client _client = widget.client ?? http.Client();
  late final bool _ownsClient = widget.client == null;
  Timer? _timer;
  ConnectionStatus _status = ConnectionStatus.checking;
  String _household = 'Pilotná domácnosť';
  String _room = 'Obývačka';
  List<GenesisDevice> _devices = [];
  String? _inventoryError;
  GenesisCommand? _lastCommand;
  String? _commandMessage;
  String? _pendingDeviceId;
  bool _readingCommand = false;
  List<GenesisCommand> _commands = [];
  String? _ledgerError;
  GenesisDiagnostics? _diagnostics;
  String? _diagnosticsError;
  static const _rooms = ['Obývačka', 'Spálňa', 'Kuchyňa'];

  @override
  void initState() {
    super.initState();
    _checkHealth();
    _timer = Timer.periodic(const Duration(seconds: 15), (_) {
      _checkHealth();
      if (_accessToken.text.isNotEmpty) _refreshDevices();
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
      final devices = await api.devices(_accessToken.text);
      if (mounted) {
        setState(() {
          _principal = principal;
          _devices = devices;
          _inventoryError = null;
        });
      }
    } catch (_) {
      if (mounted) {
        setState(() {
          _principal = null;
          _devices = [];
          _inventoryError = 'Inventár nie je dostupný. Skontrolujte spojenie a prístup.';
        });
      }
    }
    await _refreshOverview();
  }

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
            Text(_household, style: theme.textTheme.labelMedium),
            if (_principal != null) ...[
              Text('  ·  ', style: theme.textTheme.labelMedium),
              Text('Rola: ${_principal!.role}', style: theme.textTheme.labelMedium),
            ],
          ],
        ),
        const SizedBox(height: 6),
        Text(_room, style: theme.textTheme.displaySmall),
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
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const ElysiumSectionLabel('Zariadenia'),
        const SizedBox(height: 10),
        Text(
          'Inventár domácnosti. Priradenie do miestností pribudne po doplnení Genesis API.',
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
        if (_devices.isEmpty && _inventoryError == null)
          ElysiumCard(
            child: Text(
              'Zatiaľ nie sú načítané žiadne zariadenia.',
              style: theme.textTheme.bodyMedium,
            ),
          ),
        for (final device in _devices) ...[
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
                  _devices = [];
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
          child: ElysiumSectionLabel('Domácnosti'),
        ),
        const SizedBox(height: 8),
        _navTile(
          label: 'Pilotná domácnosť',
          icon: Icons.home_outlined,
          selected: _household == 'Pilotná domácnosť',
          onTap: () {
            setState(() => _household = 'Pilotná domácnosť');
            if (!wide) Navigator.pop(context);
          },
        ),
        const SizedBox(height: ElysiumLayout.sectionSpacing),
        const Padding(
          padding: EdgeInsets.symmetric(horizontal: 8),
          child: ElysiumSectionLabel('Miestnosti'),
        ),
        const SizedBox(height: 8),
        for (final room in _rooms)
          _navTile(
            label: room,
            icon: Icons.door_front_door_outlined,
            selected: _room == room,
            onTap: () {
              setState(() => _room = room);
              if (!wide) Navigator.pop(context);
            },
          ),
        const SizedBox(height: 16),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 8),
          child: Text(
            'Pilotné miestnosti sú ukážkové.',
            style: theme.textTheme.labelMedium,
          ),
        ),
      ],
    );
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
