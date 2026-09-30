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
  const GenesisApp({super.key});

  @override
  Widget build(BuildContext context) => MaterialApp(
        title: 'OS Genesis',
        debugShowCheckedModeBanner: false,
        // The app is adaptive and the panel lives inside Home Assistant, so it
        // follows the system rather than forcing one appearance.
        theme: elysiumTheme(Brightness.light),
        darkTheme: elysiumTheme(Brightness.dark),
        themeMode: ThemeMode.system,
        home: const GenesisHome(),
      );
}

enum ConnectionStatus { checking, online, offline }

class GenesisHome extends StatefulWidget {
  const GenesisHome({super.key});

  @override
  State<GenesisHome> createState() => _GenesisHomeState();
}

class _GenesisHomeState extends State<GenesisHome> {
  final _url = TextEditingController(text: genesisDefaultApiUrl());
  final _accessToken = TextEditingController();
  GenesisPrincipal? _principal;
  final _client = http.Client();
  Timer? _timer;
  ConnectionStatus _status = ConnectionStatus.checking;
  String _household = 'Pilotná domácnosť';
  String _room = 'Obývačka';
  List<GenesisDevice> _devices = [];
  String? _inventoryError;
  GenesisCommandResult? _lastCommand;
  String? _commandMessage;
  String? _pendingDeviceId;
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
  }

  Future<void> _setPower(GenesisDevice device) async {
    final base = _baseUrl();
    if (base == null || _principal?.canControlDevices != true || device.power == null) {
      setState(() => _commandMessage = 'Táto rola nemôže ovládať zariadenie.');
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
    _client.close();
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
        _pendingDeviceId != null;
    return ElysiumCard(
      child: Row(
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
    );
  }

  Widget _commandCard(GenesisCommandResult result) {
    final theme = Theme.of(context);
    final colors = ElysiumColors.of(context);
    final (text, accent, icon) = switch (result.status) {
      'device_confirmed' => (
          'Zariadenie potvrdilo zmenu',
          ElysiumColors.teal,
          Icons.check_circle_outline,
        ),
      'provider_confirmed' => (
          'Home Assistant prijal povel; zariadenie nepotvrdené',
          ElysiumColors.caution,
          Icons.cloud_done_outlined,
        ),
      'unknown' => (
          'Výsledok je neistý',
          ElysiumColors.caution,
          Icons.help_outline,
        ),
      'failed' => ('Povel zlyhal', ElysiumColors.danger, Icons.error_outline),
      'sent' => (
          'Povel odoslaný; čaká sa na potvrdenie',
          colors.gold,
          Icons.send_outlined,
        ),
      _ => (
          'Povel prijatý; čaká sa na odoslanie',
          colors.gold,
          Icons.schedule_outlined,
        ),
    };
    return ElysiumCard(
      accent: accent,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, color: accent),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(text, style: theme.textTheme.bodyLarge),
                const SizedBox(height: 4),
                Text(result.commandId, style: theme.textTheme.labelMedium),
              ],
            ),
          ),
        ],
      ),
    );
  }

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
