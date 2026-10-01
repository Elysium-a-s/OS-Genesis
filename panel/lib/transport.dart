/// Kam sa panel smie pripojiť.
///
/// Pilot hovorí HTTP, pretože jednotka stojí v domácnosti a certifikát pre
/// privátnu adresu nemá kto vydať. To je obhájiteľné **len na lokálnej sieti**:
/// tam je útočník niekto, kto už je vo vašej Wi-Fi. Na verejnej adrese je to
/// niečo úplne iné — token a povely by išli cez cudzie siete v čitateľnej
/// podobe.
///
/// Pravidlo je preto rovnaké ako to, ktoré vynucuje iOS cez
/// `NSAllowsLocalNetworking` (ELYSIUM-360): HTTP áno na lokálnu sieť, inak HTTPS.
/// Keby sa klient a platforma rozchádzali, jedna z nich by mlčky vyhrala a
/// človek by nevedel, ktorá.
library;

/// Prečo je adresa prijatá alebo odmietnutá.
enum GenesisAddressVerdict {
  /// HTTPS. Prenos je šifrovaný, nech je jednotka kdekoľvek.
  secure,

  /// HTTP na lokálnu sieť. Pilotný režim — vedome, nie omylom.
  localPilot,

  /// Adresa sa nedá prečítať ako `http(s)://host`.
  unusable,

  /// HTTP na adresu, ktorá nie je lokálna. Toto panel odmieta.
  insecureRemote,

  /// Schéma, ktorú panel nepoužíva (`ws://`, `file://`, …).
  unsupportedScheme,
}

/// Rozhodnutie o jednej adrese: čo s ňou je, a veta pre človeka.
class GenesisAddress {
  const GenesisAddress._(this.verdict, this.uri, this.explanation);

  final GenesisAddressVerdict verdict;

  /// Normalizovaná adresa, alebo `null`, keď sa použiť nedá.
  final Uri? uri;

  /// Veta, ktorá hovorí, čo sa stalo a prečo. Pri prijatej adrese vysvetľuje
  /// režim, nie že je všetko v poriadku.
  final String explanation;

  /// Či sa na túto adresu smie pripojiť.
  bool get isUsable =>
      verdict == GenesisAddressVerdict.secure ||
      verdict == GenesisAddressVerdict.localPilot;

  /// Či ide o nešifrovaný pilotný režim. Panel to má povedať nahlas.
  bool get isLocalPilot => verdict == GenesisAddressVerdict.localPilot;

  static const _unusable = GenesisAddress._(
    GenesisAddressVerdict.unusable,
    null,
    'Zadajte adresu jednotky, napríklad https://genesis.local alebo '
    'http://192.168.1.10:8080.',
  );

  /// Posúdi adresu, ktorú zadal človek.
  factory GenesisAddress.parse(String raw) {
    final trimmed = raw.trim();
    if (trimmed.isEmpty) return _unusable;
    final uri = Uri.tryParse(trimmed);
    if (uri == null || uri.host.isEmpty) return _unusable;
    if (uri.scheme == 'https') {
      return GenesisAddress._(
        GenesisAddressVerdict.secure,
        uri,
        'Prenos je šifrovaný (HTTPS).',
      );
    }
    if (uri.scheme != 'http') {
      return GenesisAddress._(
        GenesisAddressVerdict.unsupportedScheme,
        null,
        'Panel hovorí HTTP alebo HTTPS. Schému „${uri.scheme}" nepoužíva.',
      );
    }
    if (isLocalHost(uri.host)) {
      return GenesisAddress._(
        GenesisAddressVerdict.localPilot,
        uri,
        'Pilotný režim: nešifrované spojenie v rámci lokálnej siete. '
        'Token a povely nevychádzajú z vašej siete, ale kto v nej už je, ich '
        'vidí. Mimo domácnosti použite HTTPS.',
      );
    }
    return GenesisAddress._(
      GenesisAddressVerdict.insecureRemote,
      null,
      'Nešifrované spojenie na adresu „${uri.host}", ktorá nie je lokálna, '
      'panel odmieta: token a povely by išli cez cudzie siete čitateľne. '
      'Použite HTTPS, alebo adresu jednotky vo vašej sieti.',
    );
  }

  /// Či je hostiteľ na lokálnej sieti.
  ///
  /// Rozsahy sú tie, ktoré za lokálne považuje aj iOS: loopback, privátne IPv4
  /// bloky (RFC 1918), link-local (`169.254/16`), CGNAT (`100.64/10`), IPv6
  /// loopback a unique-local (`fc00::/7`), link-local (`fe80::/10`), mená v
  /// `.local` a mená bez domény.
  ///
  /// Čo sa nedá zaradiť, lokálne **nie je**. Pri adrese sa háda v prospech
  /// šifrovania, nie v prospech pohodlia.
  static bool isLocalHost(String host) {
    final name = host.toLowerCase();
    if (name == 'localhost') return true;
    final address = _parseIpv4(name);
    if (address != null) return _isPrivateIpv4(address);
    if (name.startsWith('[') && name.endsWith(']')) {
      return _isLocalIpv6(name.substring(1, name.length - 1));
    }
    if (name.contains(':')) return _isLocalIpv6(name);
    if (name.endsWith('.local')) return true;
    // Meno bez bodky je jednoznačne vnútrosieťové; verejné DNS ho nerieši.
    if (!name.contains('.')) return true;
    return false;
  }

  static List<int>? _parseIpv4(String host) {
    final parts = host.split('.');
    if (parts.length != 4) return null;
    final octets = <int>[];
    for (final part in parts) {
      if (part.isEmpty || part.length > 3) return null;
      final value = int.tryParse(part);
      if (value == null || value < 0 || value > 255) return null;
      octets.add(value);
    }
    return octets;
  }

  static bool _isPrivateIpv4(List<int> o) {
    if (o[0] == 127) return true; // loopback
    if (o[0] == 10) return true; // 10/8
    if (o[0] == 192 && o[1] == 168) return true; // 192.168/16
    if (o[0] == 172 && o[1] >= 16 && o[1] <= 31) return true; // 172.16/12
    if (o[0] == 169 && o[1] == 254) return true; // link-local
    if (o[0] == 100 && o[1] >= 64 && o[1] <= 127) return true; // CGNAT
    return false;
  }

  static bool _isLocalIpv6(String host) {
    final name = host.split('%').first.toLowerCase();
    if (name == '::1') return true;
    if (name.startsWith('fe8') ||
        name.startsWith('fe9') ||
        name.startsWith('fea') ||
        name.startsWith('feb')) {
      return true; // fe80::/10
    }
    if (name.startsWith('fc') || name.startsWith('fd')) return true; // fc00::/7
    return false;
  }
}
