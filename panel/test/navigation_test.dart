import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:genesis_panel/main.dart';

void main() {
  testWidgets('wide panel changes selected room', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(const GenesisApp());
    expect(find.text('Pilotná domácnosť'), findsWidgets);
    await tester.tap(find.text('Spálňa').first);
    await tester.pump();
    expect(find.text('Spálňa'), findsNWidgets(2));
  });
}
