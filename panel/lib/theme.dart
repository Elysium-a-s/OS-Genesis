import 'package:flutter/material.dart';

/// Elysium design tokens, ported from `Elysium/Theme/ElysiumTheme.swift` in the
/// iOS app so the panel and the app read as one product rather than two.
///
/// The values are transcribed from the Swift source, not approximated: the iOS
/// theme stores colours as floating point components, and each one below is that
/// component set converted to 8-bit. Where the app is adaptive, both variants are
/// kept, because the panel runs inside Home Assistant and has to follow the
/// system the same way the app does.
class ElysiumColors {
  const ElysiumColors({
    required this.background,
    required this.surface,
    required this.surfaceElevated,
    required this.textPrimary,
    required this.textSecondary,
    required this.textTertiary,
    required this.border,
    required this.gold,
    required this.softGold,
    required this.onAccent,
  });

  final Color background;
  final Color surface;
  final Color surfaceElevated;
  final Color textPrimary;
  final Color textSecondary;
  final Color textTertiary;
  final Color border;
  final Color gold;
  final Color softGold;

  /// Text and icons drawn on top of the gold accent.
  final Color onAccent;

  /// Brand colours that do not change between light and dark in the app.
  static const teal = Color(0xFF00F5D4);
  static const electricBlue = Color(0xFF00BFFF);
  static const purple = Color(0xFF9D4EDD);
  static const danger = Color(0xFFE64D59);

  /// A state that is neither healthy nor a failure: stale, unknown, in flight.
  static const caution = Color(0xFFFFC94D);

  static const dark = ElysiumColors(
    background: Color(0xFF0D0D0D),
    surface: Color(0xFF17171A),
    surfaceElevated: Color(0xFF212125),
    textPrimary: Color(0xFFFFFFFF),
    textSecondary: Color(0xFFB8B8B8),
    textTertiary: Color(0xFF858585),
    border: Color(0x14FFFFFF),
    gold: Color(0xFFD4AF37),
    softGold: Color(0xFFFAD98C),
    onAccent: Color(0xFF000000),
  );

  static const light = ElysiumColors(
    background: Color(0xFFF6F7F9),
    surface: Color(0xFFFFFFFF),
    surfaceElevated: Color(0xFFEDF0F4),
    textPrimary: Color(0xFF0E0F13),
    textSecondary: Color(0xFF4A4F59),
    textTertiary: Color(0xFF666B78),
    border: Color(0x1F000000),
    gold: Color(0xFF7A5900),
    softGold: Color(0xFFA87A08),
    onAccent: Color(0xFFFFFFFF),
  );

  static ElysiumColors of(BuildContext context) =>
      Theme.of(context).brightness == Brightness.dark ? dark : light;

  /// The app's signature accent fill.
  LinearGradient get goldGradient => LinearGradient(
        colors: [gold, softGold],
        begin: Alignment.topLeft,
        end: Alignment.bottomRight,
      );

  /// Gold at low opacity, for an icon well or a tinted pill.
  Color goldWash(int alpha) => elysiumAlpha(gold, alpha);
}

/// The same colour at a given alpha. The component accessors return 0–1
/// doubles, so they are scaled back to bytes here once rather than at each use.
Color elysiumAlpha(Color color, int alpha) => Color.fromARGB(
      alpha,
      (color.r * 255).round(),
      (color.g * 255).round(),
      (color.b * 255).round(),
    );

/// Mirrors `ElysiumLayout`.
class ElysiumLayout {
  static const double screenPadding = 20;
  static const double sectionSpacing = 20;
  static const double maximumContentWidth = 760;
  static const double minimumTapTarget = 44;
  static const double cardCornerRadius = 18;
  static const double controlCornerRadius = 14;
}

/// The app uses SF Rounded. It resolves on Apple platforms and in Safari; other
/// platforms fall back down this list, which is why weight, size and tracking
/// carry the identity rather than the face alone.
const List<String> elysiumFontFallback = <String>[
  'SF Pro Rounded',
  '-apple-system',
  'SF Pro Text',
  'Segoe UI Variable',
  'Roboto',
];

/// Uppercase with wide tracking, the app's section-label treatment.
const double elysiumLabelTracking = 4;

ThemeData elysiumTheme(Brightness brightness) {
  final c = brightness == Brightness.dark ? ElysiumColors.dark : ElysiumColors.light;

  TextStyle style(double size, FontWeight weight, Color color, {double? tracking}) =>
      TextStyle(
        fontFamilyFallback: elysiumFontFallback,
        fontSize: size,
        fontWeight: weight,
        color: color,
        letterSpacing: tracking,
        height: 1.25,
      );

  return ThemeData(
    useMaterial3: true,
    brightness: brightness,
    scaffoldBackgroundColor: c.background,
    colorScheme: ColorScheme.fromSeed(
      seedColor: c.gold,
      brightness: brightness,
    ).copyWith(
      surface: c.surface,
      primary: c.gold,
      onPrimary: c.onAccent,
      error: ElysiumColors.danger,
      outline: c.border,
    ),
    // Typography follows ElysiumTheme.Typography: title 34 bold, heading 20
    // semibold, body 15 regular, caption 12 medium.
    textTheme: TextTheme(
      displaySmall: style(34, FontWeight.w700, c.textPrimary),
      headlineSmall: style(24, FontWeight.w700, c.textPrimary),
      titleLarge: style(20, FontWeight.w600, c.textPrimary),
      titleMedium: style(17, FontWeight.w600, c.textPrimary),
      bodyLarge: style(15, FontWeight.w400, c.textPrimary),
      bodyMedium: style(15, FontWeight.w400, c.textSecondary),
      bodySmall: style(13, FontWeight.w400, c.textSecondary),
      labelLarge: style(15, FontWeight.w600, c.textPrimary),
      labelMedium: style(12, FontWeight.w500, c.textTertiary),
      labelSmall: style(12, FontWeight.w500, c.textTertiary, tracking: elysiumLabelTracking),
    ),
    inputDecorationTheme: InputDecorationTheme(
      filled: true,
      fillColor: c.surfaceElevated,
      labelStyle: style(13, FontWeight.w500, c.textTertiary),
      floatingLabelStyle: style(13, FontWeight.w500, c.gold),
      contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 16),
      border: OutlineInputBorder(
        borderRadius: BorderRadius.circular(ElysiumLayout.controlCornerRadius),
        borderSide: BorderSide(color: c.border),
      ),
      enabledBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(ElysiumLayout.controlCornerRadius),
        borderSide: BorderSide(color: c.border),
      ),
      focusedBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(ElysiumLayout.controlCornerRadius),
        borderSide: BorderSide(color: c.gold, width: 1.5),
      ),
    ),
    switchTheme: SwitchThemeData(
      thumbColor: WidgetStateProperty.resolveWith((states) {
        if (states.contains(WidgetState.disabled)) return c.textTertiary;
        return states.contains(WidgetState.selected) ? c.onAccent : c.surface;
      }),
      trackColor: WidgetStateProperty.resolveWith((states) {
        if (states.contains(WidgetState.disabled)) return c.surfaceElevated;
        return states.contains(WidgetState.selected) ? c.gold : c.surfaceElevated;
      }),
      trackOutlineColor: WidgetStateProperty.resolveWith(
        (states) => states.contains(WidgetState.selected) ? c.gold : c.border,
      ),
    ),
    filledButtonTheme: FilledButtonThemeData(
      style: FilledButton.styleFrom(
        backgroundColor: c.gold,
        foregroundColor: c.onAccent,
        minimumSize: const Size(0, ElysiumLayout.minimumTapTarget),
        padding: const EdgeInsets.symmetric(horizontal: 20),
        textStyle: style(15, FontWeight.w600, c.onAccent),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(ElysiumLayout.controlCornerRadius),
        ),
      ),
    ),
    outlinedButtonTheme: OutlinedButtonThemeData(
      style: OutlinedButton.styleFrom(
        foregroundColor: c.textPrimary,
        side: BorderSide(color: c.border),
        minimumSize: const Size(0, ElysiumLayout.minimumTapTarget),
        padding: const EdgeInsets.symmetric(horizontal: 20),
        textStyle: style(15, FontWeight.w600, c.textPrimary),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(ElysiumLayout.controlCornerRadius),
        ),
      ),
    ),
    dividerTheme: DividerThemeData(color: c.border, thickness: 1, space: 1),
    iconTheme: IconThemeData(color: c.textSecondary, size: 20),
  );
}

/// The app's card: surface fill, hairline border, 18pt continuous corner.
class ElysiumCard extends StatelessWidget {
  const ElysiumCard({
    super.key,
    required this.child,
    this.padding = const EdgeInsets.all(20),
    this.accent,
  });

  final Widget child;
  final EdgeInsetsGeometry padding;

  /// Tints the border and adds a soft wash, for a card that carries a state.
  final Color? accent;

  @override
  Widget build(BuildContext context) {
    final c = ElysiumColors.of(context);
    final tint = accent;
    return Container(
      padding: padding,
      decoration: BoxDecoration(
        color: tint == null
            ? c.surface
            : elysiumAlpha(tint, 20),
        borderRadius: BorderRadius.circular(ElysiumLayout.cardCornerRadius),
        border: Border.all(
          color: tint == null
              ? c.border
              : elysiumAlpha(tint, 90),
        ),
      ),
      child: child,
    );
  }
}

/// Uppercase, widely tracked section label — the app's way of opening a section.
class ElysiumSectionLabel extends StatelessWidget {
  const ElysiumSectionLabel(this.text, {super.key});

  final String text;

  @override
  Widget build(BuildContext context) => Text(
        text.toUpperCase(),
        style: Theme.of(context).textTheme.labelSmall,
      );
}

/// A small state pill: coloured dot plus label.
class ElysiumStatusPill extends StatelessWidget {
  const ElysiumStatusPill({super.key, required this.label, required this.color});

  final String label;
  final Color color;

  @override
  Widget build(BuildContext context) {
    final c = ElysiumColors.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(
        color: elysiumAlpha(color, 28),
        borderRadius: BorderRadius.circular(999),
        border: Border.all(
          color: elysiumAlpha(color, 80),
        ),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 8,
            height: 8,
            decoration: BoxDecoration(color: color, shape: BoxShape.circle),
          ),
          const SizedBox(width: 8),
          Text(
            label,
            style: TextStyle(
              fontFamilyFallback: elysiumFontFallback,
              fontSize: 12,
              fontWeight: FontWeight.w600,
              color: c.textPrimary,
            ),
          ),
        ],
      ),
    );
  }
}

/// The GENESIS wordmark, filled with the app's gold gradient.
class ElysiumWordmark extends StatelessWidget {
  const ElysiumWordmark(this.text, {super.key, this.size = 20});

  final String text;
  final double size;

  @override
  Widget build(BuildContext context) {
    final c = ElysiumColors.of(context);
    final style = TextStyle(
      fontFamilyFallback: elysiumFontFallback,
      fontSize: size,
      fontWeight: FontWeight.w700,
      letterSpacing: elysiumLabelTracking,
      color: c.gold,
    );
    return ShaderMask(
      blendMode: BlendMode.srcIn,
      shaderCallback: (bounds) => c.goldGradient.createShader(bounds),
      child: Text(text, style: style),
    );
  }
}
