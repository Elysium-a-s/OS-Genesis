# OS Genesis Pilot

Táto Home Assistant app spustí Genesis core a automaticky sa pripojí k Home Assistantu cez interný Supervisor WebSocket proxy. Nie je potrebné zadávať Home Assistant access token.

V konfigurácii app nastavte náhodný `read_token` s aspoň 32 znakmi. Slúži výhradne na čítanie `GET /v1/devices`; `GET /health` je dostupný cez Health odkaz. Dáta ledgeru sa ukladajú do `/data` a majú prežiť reštart app.

Pilotný balík nepodporuje odosielanie povelov ani Flutter panel. Port 8765 používajte iba v domácej sieti. Ak sa v inventári neobjaví svetlo alebo zásuvka, skontrolujte, či je zariadenie pridané v Home Assistante a či app v logu hlási úspešné načítanie inventára.
