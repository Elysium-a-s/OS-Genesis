#!/usr/bin/env python3
"""Home Assistant WebSocket API, len tá časť, ktorú Genesis skutočne používa.

Nie je to mock jednotky. Jednotka beží ako preložený binárny súbor a hovorí sem
po reálnom sockete tým istým protokolom ako do Home Assistanta: auth_required →
auth → auth_ok, subscribe_events, get_states, štyri registrové dotazy a
call_service. Vďaka tomu ide testovať aj to, čo sa proti mocku overiť nedá —
výpadok spojenia, obnova a najmä korelácia context.id, na ktorej stojí rozdiel
medzi `provider_confirmed` a `device_confirmed`.

Správanie sa riadi súborom (`--control`), aby ho test menil bez reštartu:

    normal          povel potvrdí a vydá state_changed s rovnakým context.id
    ack_only        povel potvrdí, ale stav nikdy nezmení → `unknown`, nie úspech
    reject          povel odmietne (success: false)

Žiaden token sa nezapisuje do logu: iba to, či autentifikácia prešla.
"""
import argparse
import asyncio
import os
import json
import pathlib
import sys
import uuid

import websockets

SUBSCRIBE_STATE = 1
REGISTRY_EVENT_IDS = {2: "area_registry_updated", 3: "device_registry_updated",
                      4: "entity_registry_updated"}

# Dve svetlá v dvoch miestnostiach a jeden vypínač. `light.hall` je zámerne
# priradené priamo entitou, nie zariadením, aby sa overilo, že priame priradenie
# vyhráva nad dedeným — to je vetva, ktorú registre robia inak.
STATES = {
    "light.living": {"state": "on", "friendly_name": "Obývačka — strop"},
    "light.hall": {"state": "off", "friendly_name": "Chodba"},
    "switch.boiler": {"state": "off", "friendly_name": "Bojler"},
    # Nepodporovaná domény: musí sa v inventári neobjaviť vôbec.
    "sensor.teplota": {"state": "21.5", "friendly_name": "Teplota"},
}

AREAS = [
    {"area_id": "obyvacka", "name": "Obývačka"},
    {"area_id": "chodba", "name": "Chodba"},
    {"area_id": "technicka", "name": "Technická miestnosť"},
]
DEVICES = [
    {"id": "dev-living", "area_id": "obyvacka"},
    {"id": "dev-hall", "area_id": "technicka"},
    {"id": "dev-boiler", "area_id": "technicka"},
]
ENTITIES = [
    {"entity_id": "light.living", "device_id": "dev-living", "area_id": None},
    # Priame priradenie prebije miestnosť zariadenia (technicka → chodba).
    {"entity_id": "light.hall", "device_id": "dev-hall", "area_id": "chodba"},
    {"entity_id": "switch.boiler", "device_id": "dev-boiler", "area_id": None},
    {"entity_id": "sensor.teplota", "device_id": "dev-living", "area_id": None},
]

clients: dict = {}


def log(*parts):
    print("stub-ha:", *parts, file=sys.stderr, flush=True)


def full_state(entity_id):
    row = STATES[entity_id]
    return {
        "entity_id": entity_id,
        "state": row["state"],
        "attributes": {"friendly_name": row["friendly_name"]},
        "last_updated": "2026-10-01T18:00:00+00:00",
        "context": {"id": row.get("context", "seed")},
    }


def control(path):
    try:
        return pathlib.Path(path).read_text().strip() or "normal"
    except OSError:
        return "normal"


async def broadcast(message):
    """Pošle udalosť každému, kto má predplatené state_changed."""
    dead = []
    for socket, subscribed in list(clients.items()):
        if SUBSCRIBE_STATE not in subscribed:
            continue
        try:
            await socket.send(json.dumps(message))
        except Exception:
            dead.append(socket)
    for socket in dead:
        clients.pop(socket, None)


def state_changed(entity_id, context_id):
    state = full_state(entity_id)
    state["context"] = {"id": context_id}
    return {
        "id": SUBSCRIBE_STATE,
        "type": "event",
        "event": {
            "event_type": "state_changed",
            "context": {"id": context_id},
            "data": {"entity_id": entity_id, "new_state": state},
        },
    }


async def session(socket, control_path):
    subscribed: set = set()
    clients[socket] = subscribed
    await socket.send(json.dumps({"type": "auth_required"}))
    try:
        greeting = json.loads(await socket.recv())
        if greeting.get("type") != "auth" or not greeting.get("access_token"):
            await socket.send(json.dumps({"type": "auth_invalid"}))
            return
        await socket.send(json.dumps({"type": "auth_ok"}))
        log("authenticated a client")
        async for raw in socket:
            request = json.loads(raw)
            rid, kind = request.get("id"), request.get("type")
            if kind == "subscribe_events":
                if request.get("event_type") == "state_changed":
                    subscribed.add(rid)
                await socket.send(json.dumps(
                    {"id": rid, "type": "result", "success": True, "result": None}))
            elif kind == "get_states":
                await socket.send(json.dumps({
                    "id": rid, "type": "result", "success": True,
                    "result": [full_state(e) for e in STATES]}))
            elif kind == "get_config":
                await socket.send(json.dumps({
                    "id": rid, "type": "result", "success": True,
                    "result": {"location_name": "Pilot", "version": "2026.9.1"}}))
            elif kind == "config/area_registry/list":
                await socket.send(json.dumps(
                    {"id": rid, "type": "result", "success": True, "result": AREAS}))
            elif kind == "config/device_registry/list":
                await socket.send(json.dumps(
                    {"id": rid, "type": "result", "success": True, "result": DEVICES}))
            elif kind == "config/entity_registry/list":
                await socket.send(json.dumps(
                    {"id": rid, "type": "result", "success": True, "result": ENTITIES}))
            elif kind == "call_service":
                await service(socket, request, rid, control_path)
            else:
                await socket.send(json.dumps({
                    "id": rid, "type": "result", "success": False,
                    "error": {"code": "unknown_command", "message": kind or ""}}))
    except websockets.exceptions.ConnectionClosed:
        pass
    finally:
        clients.pop(socket, None)


async def service(socket, request, rid, control_path):
    mode = control(control_path)
    entity_id = request.get("target", {}).get("entity_id")
    desired = "on" if request.get("service") == "turn_on" else "off"
    if mode == "reject" or entity_id not in STATES:
        log("rejecting", request.get("service"), entity_id, "mode", mode)
        await socket.send(json.dumps({
            "id": rid, "type": "result", "success": False,
            "error": {"code": "not_found", "message": "no such entity"}}))
        return
    context_id = uuid.uuid4().hex
    await socket.send(json.dumps({
        "id": rid, "type": "result", "success": True,
        "result": {"context": {"id": context_id}}}))
    if mode == "ack_only":
        # Povel sa potvrdil, ale fyzický stav nikto nepotvrdil. Jednotka z toho
        # nesmie urobiť úspech.
        log("acknowledged without a state change for", entity_id)
        return
    STATES[entity_id]["state"] = desired
    STATES[entity_id]["context"] = context_id
    await broadcast(state_changed(entity_id, context_id))
    log("served", request.get("service"), entity_id, "context", context_id)


async def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=18123)
    parser.add_argument("--control", default="/tmp/stub-ha-mode")
    # PID si zapisuje proces sám. `setsid ... &` vráti PID setsidu, ktorý sa
    # odforkuje, takže zvonka zapísané číslo po chvíli nepatrí nikomu — a test,
    # ktorý ním „vypne" Home Assistanta, potom nevypne nič a výpadok si vymyslí.
    parser.add_argument("--pidfile")
    args = parser.parse_args()
    if args.pidfile:
        pathlib.Path(args.pidfile).write_text(str(os.getpid()))
    async with websockets.serve(lambda s: session(s, args.control),
                                "127.0.0.1", args.port):
        log("listening on 127.0.0.1:%d" % args.port)
        await asyncio.Future()


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
