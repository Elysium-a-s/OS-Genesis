import 'dart:async';

import 'package:flutter/material.dart';
import 'package:http/http.dart' as http;

void main() => runApp(const GenesisApp());

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
  final _url = TextEditingController(text: const String.fromEnvironment(
    'GENESIS_API_URL',
    defaultValue: 'http://localhost:8765',
  ));
  final _client = http.Client();
  Timer? _timer;
  ConnectionStatus _status = ConnectionStatus.checking;
  String _household = 'Pilotná domácnosť';
  String _room = 'Obývačka';
  static const _rooms = ['Obývačka', 'Spálňa', 'Kuchyňa'];

  @override
  void initState() {
    super.initState();
    _checkHealth();
    _timer = Timer.periodic(const Duration(seconds: 15), (_) => _checkHealth());
  }

  Future<void> _checkHealth() async {
    final raw = _url.text.trim();
    final base = Uri.tryParse(raw);
    if (base == null || (base.scheme != 'http' && base.scheme != 'https') || base.host.isEmpty) {
      if (mounted) setState(() => _status = ConnectionStatus.offline);
      return;
    }
    if (mounted) setState(() => _status = ConnectionStatus.checking);
    try {
      final response = await _client.get(base.resolve('/health')).timeout(const Duration(seconds: 4));
      if (mounted) setState(() => _status = response.statusCode == 200 ? ConnectionStatus.online : ConnectionStatus.offline);
    } catch (_) {
      if (mounted) setState(() => _status = ConnectionStatus.offline);
    }
  }

  @override
  void dispose() {
    _timer?.cancel();
    _client.close();
    _url.dispose();
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
                const SizedBox(height: 8),
                Text(_room, style: Theme.of(context).textTheme.displaySmall),
                const SizedBox(height: 24),
                Card(
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
                          decoration: const InputDecoration(labelText: 'Adresa Genesis API', border: OutlineInputBorder()),
                          onSubmitted: (_) => _checkHealth(),
                        ),
                        const SizedBox(height: 12),
                        FilledButton.icon(
                          onPressed: _checkHealth,
                          icon: const Icon(Icons.refresh),
                          label: const Text('Skontrolovať spojenie'),
                        ),
                      ],
                    ),
                  ),
                ),
                const SizedBox(height: 20),
                const Card(
                  child: Padding(
                    padding: EdgeInsets.all(20),
                    child: Text('Zariadenia sa zobrazia po pripojení inventára. Ovládanie bude dostupné po dokončení API príkazov.'),
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
      drawer: wide ? null : Drawer(child: SafeArea(child: rail)),
    );
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
          const Text('Pilotné miestnosti sú ukážkové. Skutočné miestnosti načíta ďalšia etapa z Genesis API.'),
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
