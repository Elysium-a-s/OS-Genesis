# OS Genesis Pilot

Táto Home Assistant app spustí Genesis core a automaticky sa pripojí k Home Assistantu cez interný Supervisor WebSocket proxy. Nie je potrebné zadávať Home Assistant access token.

V konfigurácii app nastavte dva rôzne náhodné tokeny `read_token` a `write_token`, každý s aspoň 32 znakmi. Voliteľné `member_token` a `guest_token` môžu zostať prázdne. `read_token` slúži na čítanie, `write_token` aj na pilotné ovládanie. Dáta ledgeru sa ukladajú do `/data` a majú prežiť reštart app.

Panel otvoríte tlačidlom **OPEN WEB UI** v Home Assistante aj pri vzdialenom prístupe do HA. V paneli zadajte Genesis `write_token`; adresa API sa predvyplní podľa adresy otvoreného panelu. Ingress a jeho prihlásenie zabezpečuje HA, Genesis token naďalej chráni príkazy. Port 8765 už nie je publikovaný. Ak sa v inventári neobjaví svetlo alebo zásuvka, skontrolujte, či je zariadenie pridané v Home Assistante a či app v logu hlási úspešné načítanie inventára.
