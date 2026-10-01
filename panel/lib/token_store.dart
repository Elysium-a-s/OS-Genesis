import 'package:flutter_secure_storage/flutter_secure_storage.dart';

/// Kde žije vydaný prístupový token.
///
/// Párovací kód sa dá uplatniť práve raz, takže token nemôže žiť len v pamäti
/// otvoreného panelu: po obnovení stránky by člen o prístup prišiel a nový kód
/// mu nemá kto vydať okrem vlastníka. Preto sa ukladá.
///
/// A preto je to rozhranie, nie jedno konkrétne úložisko — čo je „bezpečné"
/// závisí od platformy a na jednej z nich to nie je bezpečné vôbec.
abstract class GenesisTokenStore {
  Future<String?> read();
  Future<void> write(String token);
  Future<void> clear();
}

/// Token iba v pamäti.
///
/// Používajú to testy a je to zároveň správna voľba tam, kde trvalé uloženie
/// nikto nechce: token zmizne so zatvorením panelu.
class InMemoryTokenStore implements GenesisTokenStore {
  String? _token;

  @override
  Future<String?> read() async => _token;

  @override
  Future<void> write(String token) async => _token = token;

  @override
  Future<void> clear() async => _token = null;
}

/// Platformové úložisko tokenu.
///
/// Na iOS a iPadOS je to Keychain — skutočné zabezpečené úložisko mimo procesu
/// aplikácie, chránené systémom.
///
/// **Vo webe to zabezpečené úložisko nie je** a nikto by to tak nemal čítať.
/// `flutter_secure_storage` tam hodnotu zašifruje, ale kľúč uloží do toho istého
/// `localStorage`, takže skript s rovnakým pôvodom sa dostane k obom. Skutočnou
/// hranicou webového panelu je prihlásenie do Home Assistanta pred Ingressom a
/// pôvod stránky, nie toto úložisko. Je to napísané takto priamo preto, že z
/// názvu balíka by sa dalo vyčítať viac, než platí.
class SecureTokenStore implements GenesisTokenStore {
  SecureTokenStore({FlutterSecureStorage? storage})
      : _storage = storage ?? const FlutterSecureStorage();

  static const _key = 'genesis_access_token';

  final FlutterSecureStorage _storage;

  /// Chýbajúce úložisko nesmie zhodiť panel. Na platforme bez plugin-u (a v
  /// testoch) sa to správa ako prázdne úložisko, čo je pravda o tom, čo vieme.
  @override
  Future<String?> read() async {
    try {
      return await _storage.read(key: _key);
    } catch (_) {
      return null;
    }
  }

  @override
  Future<void> write(String token) async {
    try {
      await _storage.write(key: _key, value: token);
    } catch (_) {
      // Token zostane v pamäti panelu; po obnovení stránky bude treba nový kód.
    }
  }

  @override
  Future<void> clear() async {
    try {
      await _storage.delete(key: _key);
    } catch (_) {
      // Nič neuložené znamená nič na zmazanie.
    }
  }
}
