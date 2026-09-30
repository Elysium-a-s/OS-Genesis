import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/foundation.dart';
import 'package:http/http.dart' as http;

import 'genesis_api.dart';

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
        theme: ThemeData(
          useMaterial3: true,
          brightness: Brightness.dark,
          colorSchemeSeed: const Color(0xFF20D6C7),
          scaffoldBackgroundColor: const Color(0xFF090D16),
        ),
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
    final rail = _navigation(wide);
    return Scaffold(
      appBar: AppBar(
        title: const Text('GENESIS'),
        actions: [
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16),
            child: Center(child: _statusChip()),
          ),
        ],
      ),
      body: Row(
        children: [
          if (wide) SizedBox(width: 240, child: rail),
          Expanded(
            child: ListView(
              padding: EdgeInsets.all(wide ? 32 : 16),
              children: [
                Text(_household, style: Theme.of(context).textTheme.headlineSmall),
                if (_principal != null) Text('Rola: ${_principal!.role}'),
                const SizedBox(height: 8),
                Text(_room, style: Theme.of(context).textTheme.displaySmall),
                const SizedBox(height: 24),
                _connectionCard(),
                const SizedBox(height: 20),
                Text('Zariadenia', style: Theme.of(context).textTheme.headlineSmall),
                const SizedBox(height: 8),
                const Text('Inventár domácnosti. Priradenie do miestností pribudne po doplnení Genesis API.'),
                const SizedBox(height: 12),
                if (_inventoryError != null)
                  Text(_inventoryError!, style: const TextStyle(color: Colors.amberAccent)),
                if (_devices.isEmpty && _inventoryError == null)
                  const Card(child: Padding(
                    padding: EdgeInsets.all(20),
                    child: Text('Zatiaľ nie sú načítané žiadne zariadenia.'),
                  )),
                ..._devices.map(_deviceCard),
                if (_pendingDeviceId != null)
                  const Card(child: Padding(
                    padding: EdgeInsets.all(20),
                    child: Text('Povel sa spracúva. Stav zariadenia ešte nie je potvrdený.'),
                  )),
                if (_lastCommand != null) _commandCard(_lastCommand!),
                if (_commandMessage != null)
                  Card(child: Padding(
                    padding: const EdgeInsets.all(20),
                    child: Text(_commandMessage!),
                  )),
              ],
            ),
          ),
        ],
      ),
      drawer: wide ? null : Drawer(child: SafeArea(child: rail)),
    );
  }

  Widget _connectionCard() => Card(
        child: Padding(
          padding: const EdgeInsets.all(20),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text('Spojenie s Genesis API'),
              const SizedBox(height: 12),
              TextField(
                controller: _url,
                keyboardType: TextInputType.url,
                decoration: const InputDecoration(
                  labelText: 'Adresa Genesis API',
                  border: OutlineInputBorder(),
                ),
                onSubmitted: (_) => _checkHealth(),
              ),
              const SizedBox(height: 12),
              TextField(
                controller: _accessToken,
                obscureText: true,
                decoration: const InputDecoration(
                  labelText: 'Prístupový token',
                  border: OutlineInputBorder(),
                ),
                onChanged: (_) => setState(() {
                  _principal = null;
                  _devices = [];
                }),
              ),
              const SizedBox(height: 12),
              Wrap(spacing: 12, children: [
                FilledButton.icon(
                  onPressed: _checkHealth,
                  icon: const Icon(Icons.refresh),
                  label: const Text('Skontrolovať spojenie'),
                ),
                OutlinedButton.icon(
                  onPressed: _refreshDevices,
                  icon: const Icon(Icons.devices),
                  label: const Text('Načítať zariadenia'),
                ),
              ]),
            ],
          ),
        ),
      );

  Widget _deviceCard(GenesisDevice device) {
    final stale = device.isStale;
    final state = stale
        ? 'Stav zastaraný alebo neznámy'
        : device.power == true
            ? 'Zapnuté'
            : 'Vypnuté';
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(20),
        child: Row(children: [
          const Icon(Icons.lightbulb_outline),
          const SizedBox(width: 16),
          Expanded(child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(device.name, style: Theme.of(context).textTheme.titleMedium),
              Text(state, style: TextStyle(
                color: stale ? Colors.amberAccent : Colors.greenAccent,
              )),
              if (device.observedAt != null)
                Text('Posledná zmena v HA: ${device.observedAt!.toLocal()}'),
            ],
          )),
          Switch(
            value: device.power ?? false,
            onChanged: stale || !device.writable ||
                    _principal?.canControlDevices != true || _pendingDeviceId != null
                ? null
                : (_) => _setPower(device),
          ),
        ]),
      ),
    );
  }

  Widget _commandCard(GenesisCommandResult result) {
    final text = switch (result.status) {
      'device_confirmed' => 'Zariadenie potvrdilo zmenu',
      'provider_confirmed' => 'Home Assistant prijal povel; zariadenie nepotvrdené',
      'unknown' => 'Výsledok je neistý',
      'failed' => 'Povel zlyhal',
      'sent' => 'Povel odoslaný; čaká sa na potvrdenie',
      _ => 'Povel prijatý; čaká sa na odoslanie',
    };
    return Card(child: Padding(
      padding: const EdgeInsets.all(20),
      child: Text('$text (${result.commandId})'),
    ));
  }

  Widget _navigation(bool wide) => ListView(
        padding: const EdgeInsets.all(16),
        children: [
          const Text('DOMÁCNOSTI'),
          const SizedBox(height: 8),
          ListTile(
            title: const Text('Pilotná domácnosť'),
            leading: const Icon(Icons.home_outlined),
            selected: _household == 'Pilotná domácnosť',
            onTap: () {
              setState(() => _household = 'Pilotná domácnosť');
              if (!wide) Navigator.pop(context);
            },
          ),
          const Divider(),
          const Text('MIESTNOSTI'),
          ..._rooms.map((room) => ListTile(
                title: Text(room),
                leading: const Icon(Icons.door_front_door_outlined),
                selected: _room == room,
                onTap: () {
                  setState(() => _room = room);
                  if (!wide) Navigator.pop(context);
                },
              )),
          const SizedBox(height: 12),
          const Text('Pilotné miestnosti sú ukážkové.'),
        ],
      );

  Widget _statusChip() {
    final (label, color) = switch (_status) {
      ConnectionStatus.checking => ('Overujem', Colors.amber),
      ConnectionStatus.online => ('Online', Colors.greenAccent),
      ConnectionStatus.offline => ('Offline', Colors.redAccent),
    };
    return Chip(label: Text(label), avatar: Icon(Icons.circle, size: 12, color: color));
  }
}
