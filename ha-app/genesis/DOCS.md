# OS Genesis Pilot

Táto Home Assistant app spustí Genesis core a automaticky sa pripojí k Home Assistantu cez interný Supervisor WebSocket proxy. Nie je potrebné zadávať Home Assistant access token.

V konfigurácii app nastavte dva rôzne náhodné tokeny `read_token` a `write_token`, každý s aspoň 32 znakmi. Voliteľné `member_token` a `guest_token` môžu zostať prázdne. `read_token` slúži na čítanie, `write_token` aj na pilotné ovládanie. Dáta ledgeru sa ukladajú do `/data` a majú prežiť reštart app.

Ostatné možnosti sú voliteľné a prázdne znamenajú vypnuté:

| Možnosť | Čo zapína | Keď zostane prázdna |
| --- | --- | --- |
| `sensitive_devices` | zariadenia, ktoré pri hlasovom povele vyžadujú potvrdenie, oddelené čiarkou (napr. `ha:switch.boiler, ha:switch.gate`) | žiadne zariadenie nie je citlivé a hlasový povel ide hneď |
| `behavior_key_id`, `behavior_secret` | podpísaný kanál z Elysium Behavior na `POST /v1/behavior/decisions`; tajomstvo má aspoň 32 znakov | kanál je vypnutý a endpoint odpovedá `behavior_channel_not_configured` |
| `central_unit_id` | jednotka prijme iba rozhodnutia určené jej UUID | jednotka prijme rozhodnutie pre ktorúkoľvek jednotku vo svojej domácnosti |

`behavior_key_id` a `behavior_secret` platia len spolu — nastaviť jedno bez druhého app neodštartuje, pretože polovične nastavený kanál znamená, že ho niekto zapnúť chcel.

Zálohy sa od verzie `0.1.4` ukladajú do `/data/backups`, takže `POST /v1/backup` funguje bez ďalšieho nastavenia a záloha prežije reštart aj aktualizáciu app. Postup obnovy je v [docs/ELYSIUM-348-obnova.md](https://github.com/Elysium-a-s/OS-Genesis/blob/main/docs/ELYSIUM-348-obnova.md).

Panel otvoríte tlačidlom **OPEN WEB UI** v Home Assistante aj pri vzdialenom prístupe do HA. V paneli zadajte Genesis `write_token`; adresa API sa predvyplní podľa adresy otvoreného panelu. Ingress a jeho prihlásenie zabezpečuje HA, Genesis token naďalej chráni príkazy. Port 8765 už nie je publikovaný. Ak sa v inventári neobjaví svetlo alebo zásuvka, skontrolujte, či je zariadenie pridané v Home Assistante a či app v logu hlási úspešné načítanie inventára.
