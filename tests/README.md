# Testy a pilot

Sem patria end-to-end scenáre a dôkazy z fyzických zariadení.

* `ui-acceptance/` — UI akceptačný test (ELYSIUM-362). Panel v skutočnom
  prehliadači proti preloženej jednotke; na druhej strane server hovoriaci
  WebSocket protokolom Home Assistanta. Pokrýva načítanie stavu svetla, povel,
  potvrdenie aj explicitne neznámy výsledok, výpadok Home Assistanta a obnovu
  spojenia. Výsledky posledného behu sú v
  `docs/ELYSIUM-362-ui-akceptacny-test.md`.

Dôkazy z fyzického Home Assistant Greenu, žiarovky a iPadu tu zatiaľ nie sú —
to je ELYSIUM-350 a žiadny z testov v tomto adresári ho nenahrádza.
