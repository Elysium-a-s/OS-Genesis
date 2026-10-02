# UI akceptačný test (ELYSIUM-362)

Prejde panelom v skutočnom prehliadači proti skutočnej jednotke. Deväť
testovacích prípadov z ELYSIUM-362, každý s rozsudkom a dôvodom.

```sh
sh tests/ui-acceptance/run.sh
```

Potrebuje `cargo`, `flutter`, `python3` s `websockets`, `node` s `playwright`
a Chromium. Cestu k Chromiu vie prevziať z `CHROMIUM_BIN`, k Flutteru z
`FLUTTER_BIN`. `GENESIS_SKIP_BUILD=1` preskočí preklad, `GENESIS_ONLY=3,5`
spustí len vybrané prípady.

## Prečo takto

Panel má 57 widget testov a jednotka 119 testov. Všetkých 57 ale mockuje HTTP
klienta: overujú, že panel požiadavku správne **zloží**, nie že na ňu jednotka
odpovie. Testy jednotky idú z druhej strany. Medzi nimi je medzera, do ktorej sa
zmestí nekompatibilný tvar odpovede — a prišlo by sa naň až pri zapojení
skutočného Home Assistant Greenu.

Tento test tú medzeru zatvára: panel je skutočný `flutter build web --release`
v Chromiu, jednotka je preložená binárka a hovoria spolu po sieti.

Na druhej strane jednotky stojí `stub_ha.py` — nie mock vnútra jednotky, ale
WebSocket server, ktorý hovorí protokolom Home Assistanta (`auth_required` →
`auth_ok`, `subscribe_events`, `get_states`, štyri registrové dotazy,
`call_service`). Vďaka tomu sa dá otestovať aj to, čo sa proti mocku nedá:

* **výpadok a obnova** — spojenie sa naozaj preseče (prípad 4),
* **korelácia `context.id`** — rozdiel medzi „povel prijatý" a „stav potvrdený",
  teda medzi `provider_confirmed` a `device_confirmed` (prípad 5),
* **nepotvrdený stav** — Home Assistant povel prijme a stav nezmení; jednotka
  z toho nesmie urobiť úspech (prípad 5, režim `ack_only`).

## Ako sa číta obrazovka

Flutter kreslí do canvasu, takže v DOM nie je text. Test zapne **strom
prístupnosti** — ten istý, ktorý čítajú čítačky obrazovky, nie testovací zadný
vchod. Má to dva dôsledky, ktoré stáli najviac času a sú preto v kóde
okomentované:

* Pole sa musí kliknúť a vypísať, nie „vyplniť". Flutter zosúlaďuje text s DOM
  len pre pole s fokusom; `fill()` bez fokusu odovzdá zmazanie a nový text už
  nie, takže adresa zostane prázdna, tlačidlo zakázané a vyzerá to ako chyba
  panela.
* Strom sa po prestavbe plní postupne a občas sa celý zruší. Čítanie preto čaká,
  kým sa ustáli, a tlačidlá sa hľadajú podľa presného vlastného textu — `hasText`
  trafí aj rodičovský uzol, ktorého kliknutie nič nespraví.

## Čo tento test nedokazuje

Fyzickú žiarovku, fyzický iPad a skutočný Home Assistant Green. To je
**ELYSIUM-350** a nahradiť sa to nedá. Zelený beh tu znamená, že softvér robí,
čo tvrdí — nie že to niekto videl v domácnosti.

Tokeny a tajomstvo Behavior kanála v `start-unit.sh` sú testovacie hodnoty pre
jeden beh. Certifikáty ani kódy sa nikam neukladajú; párovací kód test číta
z obrazovky a do výstupu ho nepíše.
