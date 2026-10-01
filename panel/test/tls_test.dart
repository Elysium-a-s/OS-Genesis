import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:genesis_panel/genesis_api.dart';

/// Overenie certifikátu, v samostatnom súbore a bez jediného widget testu.
///
/// `flutter_test` pri inicializácii widget bindingu nastaví `HttpOverrides.global`
/// na klienta, ktorý na **každú** požiadavku odpovie HTTP 400 — aby testy
/// nechodili do siete. Keby tento test býval v súbore s `testWidgets`, binding by
/// sa stihol inicializovať a namiesto chyby certifikátu by prišlo 400: test by
/// prešiel na nesprávnom dôvode, alebo spadol na nezrozumiteľnom. Stálo ma to
/// jedno zmätené kolo, kým som to našiel.
void main() {
  /// Neplatný certifikát musí spojenie zastaviť, nie prejsť.
  ///
  /// Je to skutočný TLS server s vlastným podpisom, nie mock: práve túto
  /// vlastnosť sa nedá overiť predstieraním, pretože ju vynucuje knižnica pod
  /// klientom. Keby niekto vypol overovanie certifikátu, nič iné by to nezachytilo.
  ///
  /// Certifikát sa **generuje pri teste**, neleží v repozitári: podpisový
  /// materiál do gitu nepatrí ani keď je bezcenný, a `.gitignore` to hovorí tiež.
  test('a self-signed certificate stops the connection', () async {
    final directory =
        await Directory.systemTemp.createTemp('genesis-tls-test-');
    addTearDown(() => directory.delete(recursive: true));
    final certificate = '${directory.path}/cert.pem';
    final key = '${directory.path}/key.pem';

    final openssl = await Process.run('openssl', [
      'req', '-x509', '-newkey', 'rsa:2048', '-nodes',
      '-keyout', key, '-out', certificate,
      '-days', '1', '-subj', '/CN=localhost',
      '-addext', 'subjectAltName=IP:127.0.0.1',
    ]);
    if (openssl.exitCode != 0) {
      // Bez openssl sa táto vlastnosť overiť nedá a predstierať sa nebude.
      markTestSkipped('openssl nie je dostupné: ${openssl.stderr}');
      return;
    }

    final context = SecurityContext()
      ..useCertificateChain(certificate)
      ..usePrivateKey(key);
    final server =
        await HttpServer.bindSecure(InternetAddress.loopbackIPv4, 0, context);
    addTearDown(() => server.close(force: true));
    server.listen((request) {
      request.response
        ..statusCode = 200
        ..write('{"status":"ok"}');
      request.response.close();
    });

    final api = GenesisApi(
      baseUrl: Uri.parse('https://127.0.0.1:${server.port}'),
    );
    addTearDown(api.close);

    // Certifikát nepodpísal nikto, komu klient verí, takže sa spojenie
    // nedokončí. Keby sa dokončilo, panel by veril komukoľvek.
    await expectLater(
      api.me('read-token-of-at-least-32-characters'),
      throwsA(isA<HandshakeException>()),
    );

    // A ten istý server s dôverovaným certifikátom by prešiel — takže test
    // nemeria len to, že spojenie zlyhá, ale že zlyhá na certifikáte.
    final trusting = HttpClient(context: SecurityContext(withTrustedRoots: false))
      ..badCertificateCallback = (_, __, ___) => true;
    addTearDown(() => trusting.close(force: true));
    final probe = await trusting
        .getUrl(Uri.parse('https://127.0.0.1:${server.port}/health'));
    final response = await probe.close();
    expect(response.statusCode, 200);
  });
}
