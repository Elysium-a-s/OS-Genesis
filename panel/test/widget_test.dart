import 'package:flutter_test/flutter_test.dart';
import 'package:genesis_panel/main.dart';

void main() {
  testWidgets('panel shows Genesis identity and connection status', (tester) async {
    await tester.pumpWidget(const GenesisApp());
    expect(find.text('GENESIS'), findsOneWidget);
    expect(find.text('Spojenie s Genesis API'), findsOneWidget);
  });
}
