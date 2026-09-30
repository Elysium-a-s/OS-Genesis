# Dokumentácia

Sem patria potvrdené rozhodnutia architektúry, postup nasadenia, prevádzka a diagnostika. Koncepčné podklady sú zatiaľ v priečinku [OS Genesis v Confluence](https://sarockylukas.atlassian.net/wiki/spaces/Elysium/folder/46891009/OS+Genesis). Rozhodnutia z pilotu sa sem prenesú s dátumom a dôkazom.

## Pilotný hardvér

- [ELYSIUM-332 — inventár Home Assistant Green](ELYSIUM-332-pilot-hardware.md): potvrdené parametre modelu, otvorené údaje konkrétnej jednotky a brána dokončenia.

## Bezpečnosť

- [ELYSIUM-347 — párovanie, roly a správa tajomstiev](ELYSIUM-347-identita.md): čomu má návrh zabrániť, ako to rieši, a čo zostáva otvorené.

## Prevádzka

- [ELYSIUM-348 — záloha, aktualizácia a obnova](ELYSIUM-348-obnova.md): postup pre prevádzkovateľa, čo Genesis po reštarte urobí sám, a čo nie je odmerané.
- [ELYSIUM-352 — inštalácia, aktualizácia a rollback HA app](ELYSIUM-352-instalacia.md): zvolená distribučná cesta, prečo Supervisor novú verziu nevidí hneď, a prečo rollback potrebuje zálohu spravenú pred aktualizáciou.

## Protokoly

- [ELYSIUM-349 — Matter a Thread na pilotnom hardvéri](ELYSIUM-349-matter-thread.md): rádiový hardvér Green, čo z Matteru funguje už dnes cez Home Assistant, možnosti SDK, a rozhodnutie odložiť natívnu implementáciu. Podpora nie je deklarovaná — fyzický test je rozpísaný, ale neprebehol.
