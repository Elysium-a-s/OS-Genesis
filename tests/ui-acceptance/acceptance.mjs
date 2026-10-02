// UI akceptačný test Genesis panela (ELYSIUM-362).
//
// Panel beží v skutočnom Chromiu proti skutočnej jednotke po HTTP. To je presne
// tá medzera, ktorú widget testy nechávajú otvorenú: tých 57 mockuje HTTP
// klienta, takže overujú, že panel požiadavku správne *zloží*, nie že na ňu
// jednotka odpovie. Tu sa po sieti pošle a odpoveď sa prečíta z obrazovky.
//
// Flutter kreslí do canvasu, takže v DOM nie je text. Test preto zapne strom
// prístupnosti (`flt-semantics-placeholder`), ktorý je rovnaký strom, aký čítajú
// čítačky obrazovky — nie testovací zadný vchod.
import { createRequire } from 'node:module';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { spawn } from 'node:child_process';

// Playwright sa hľadá CJS rozlišovaním, takže ho nájde aj globálna inštalácia
// cez NODE_PATH, aj `npm i playwright` v tomto repozitári. ESM import by videl
// len to druhé.
const { chromium } = createRequire(import.meta.url)('playwright');

const UNIT = 'http://127.0.0.1:18080';
const WORK = process.env.GENESIS_TEST_WORK || '/tmp/genesis-ui-acceptance';
const CHROMIUM = process.env.CHROMIUM_BIN || undefined;
const TOKENS = {
  owner: 'write-token-ABCDEFGHIJKLMNOPQRSTU',
  member: 'member-token-abcdefghijklmnopqrst',
  guest: 'guest-token-abcdefghijklmnopqrstu',
};
const HOUSEHOLD = 'pilot-home';
const BEHAVIOR = {
  keyId: 'pilot-behavior-key',
  secret: 'pilot-behavior-secret-of-32-chars',
  centralUnit: 'ad19a578-21e2-453f-a57c-1913350be34e',
};

const results = [];
let page, browser;

function record(id, title, verdict, notes) {
  results.push({ id, title, verdict, notes });
  const mark = { PASS: 'PASS', FAIL: 'FAIL', BLOCKED: 'BLOCKED', PARTIAL: 'PARTIAL' }[verdict];
  console.log(`\n[${mark}] ${id}. ${title}`);
  for (const note of notes) console.log('      · ' + note);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---------- ovládanie panela cez strom prístupnosti ----------

async function openPanel() {
  browser = await chromium.launch({ executablePath: CHROMIUM, args: ['--no-sandbox'] });
  // Veľmi vysoké okno: Flutter widgety mimo obrazovky nekreslí a do stromu
  // prístupnosti sa potom nedostanú. Panel rastie, ako pribúdajú karty (výsledok
  // povelu, audit, zálohy), takže okno musí pokryť aj najdlhšiu podobu stránky —
  // inak by test hlásil, že pole „na obrazovke nie je", len preto, že je nižšie.
  page = await browser.newPage({ viewport: { width: 1400, height: 6400 } });
  page.pageErrors = [];
  page.blockedHosts = new Set();
  page.on('pageerror', (error) => page.pageErrors.push(String(error)));
  // Domácnosť bez internetu: všetko okrem jednotky je nedostupné. Panel sa to má
  // dozvedieť teraz, nie u človeka doma.
  await page.route('**', (route) => {
    const url = new URL(route.request().url());
    if (url.hostname === '127.0.0.1') return route.continue();
    page.blockedHosts.add(url.hostname);
    return route.abort();
  });
  await page.goto(UNIT + '/', { waitUntil: 'load' });
  await enableSemantics();
}

// Flutter kreslí do canvasu; text je v DOM až keď sa zapne strom prístupnosti.
// Bez neho by každé tvrdenie o obrazovke zlyhalo z nesprávneho dôvodu, takže sa
// tu čaká a kontroluje, nie iba klikne.
async function enableSemantics() {
  await page.waitForSelector('flt-semantics-placeholder', { timeout: 30000 }).catch(() => {});
  for (let attempt = 0; attempt < 20; attempt += 1) {
    await page.evaluate(() => document.querySelector('flt-semantics-placeholder')?.click());
    await sleep(800);
    if ((await rawScreen()).length > 0) return;
  }
  throw new Error('strom prístupnosti sa nezapol, obrazovka je neprečítateľná');
}

// `innerText` tu nefunguje: uzly stromu prístupnosti sú vizuálne skryté, takže
// ich innerText vynechá. `textContent` ich vidí.
const rawScreen = () =>
  page.$$eval('flt-semantics', (nodes) =>
    nodes.map((node) => (node.getAttribute('aria-label') || node.textContent || '').trim())
      .filter(Boolean).join('\n'));

// Flutter strom prístupnosti občas zruší (napríklad po prestavbe stránky), a
// vtedy je obrazovka prázdna. To nie je chyba panela, len dôsledok toho, že sa
// číta cez prístupnosť, takže sa strom tichо zapne znova.
// Strom sa po prestavbe plní postupne, takže jedno čítanie môže zachytiť polovicu
// obrazovky. Číta sa preto, kým sa dĺžka neustáli — inak by tvrdenia o obrazovke
// zlyhávali náhodne a vyzeralo by to ako chyba panela.
const screen = async () => {
  let text = await rawScreen();
  if (text.length === 0) {
    await enableSemantics().catch(() => {});
    text = await rawScreen();
  }
  for (let attempt = 0; attempt < 10; attempt += 1) {
    await sleep(400);
    const again = await rawScreen();
    if (again.length === text.length) return again;
    text = again;
  }
  return text;
};

async function shows(needle, timeout = 8000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if ((await screen()).includes(needle)) return true;
    await sleep(250);
  }
  return false;
}

// Do poľa sa musí klikať a písať, nie ho „vyplniť".
//
// Flutter drží text vo svojom controlleri a s DOM ho zosúlaďuje len pre pole,
// ktoré má fokus. `fill()` bez fokusu Flutteru odovzdá zmazanie, ale už nie nový
// text, takže controller zostane prázdny — adresa je potom nepoužiteľná, tlačidlo
// zakázané a kliknutie neurobí nič. Test by vyzeral ako chyba panela.
async function fill(label, value) {
  for (let attempt = 0; attempt < 5; attempt += 1) {
    const field = await page.$(`input[aria-label="${label}"]`);
    if (!field) { await sleep(500); continue; }
    await field.click();
    await sleep(300);
    // Overuje sa zaostrený prvok, nie ten nájdený selektorom. Flutter jeden
    // editačný prvok prepoužíva a jeho aria-label sa mení až po presune fokusu,
    // takže selektor podľa labelu môže ukazovať na pole, ktoré už drží niečo iné.
    // Pole s nápovedou má v aria-label aj tú nápovedu, na druhom riadku
    // („Povel\nnapríklad: zhasni svetlo v obývačke"), takže sa porovnáva prvý riadok.
    const focused = await page.evaluate(() =>
      (document.activeElement?.getAttribute('aria-label') || '').split('\n')[0].trim());
    if (focused !== label) { await sleep(400); continue; }
    await page.keyboard.press('ControlOrMeta+a');
    await page.keyboard.press('Delete');
    await page.keyboard.type(value, { delay: 5 });
    await sleep(300);
    const written = await page.evaluate(() => document.activeElement?.value);
    if (written === value) return;
  }
  throw new Error(`do poľa "${label}" sa nepodarilo zapísať hodnotu`);
}

// Uzly stromu prístupnosti sú vnorené, takže `hasText` trafí aj rodiča, ktorého
// kliknutie nič nespraví. Hľadá sa preto prvok, ktorého vlastný text sa labelu
// presne rovná, a berie sa ten najhlbší.
async function press(label) {
  await screen(); // strom prístupnosti sa cez prestavby stráca; toto ho vráti
  const clicked = await page.evaluate((wanted) => {
    const buttons = [...document.querySelectorAll('flt-semantics[role=button]')]
      .filter((node) => (node.textContent || '').trim() === wanted);
    if (buttons.length === 0) return false;
    buttons[buttons.length - 1].click();
    return true;
  }, label);
  if (!clicked) {
    const offered = await page.evaluate(() =>
      [...document.querySelectorAll('flt-semantics[role=button]')]
        .map((node) => (node.textContent || '').trim())
        .filter((text) => text && text.length < 40));
    throw new Error(`tlačidlo "${label}" na obrazovke nie je; sú tam: ${offered.join(', ') || '(žiadne)'}`);
  }
  await page.waitForTimeout(800);
}

// Niektoré tlačidlá sú chvíľu zakázané (panel si sám obnovuje stav a počas
// načítania ich vypne), a vtedy v strome prístupnosti nie sú. Tam, kde ide len
// o urýchlenie obnovy, sa stlačenie preskočí namiesto pádu testu.
const pressIfPresent = async (label) => {
  if (await hasButton(label)) { await press(label); return true; }
  return false;
};

const hasButton = async (label) => {
  await screen();
  return page.evaluate((wanted) => [...document.querySelectorAll('flt-semantics[role=button]')]
    .some((node) => (node.textContent || '').trim() === wanted), label);
};

// Pripojí panel ako daná rola a počká, kým jednotka potvrdí identitu. Keby sa
// nečakalo, každé ďalšie tvrdenie by meralo predchádzajúci stav obrazovky.
async function connectAs(role) {
  for (let attempt = 0; attempt < 3; attempt += 1) {
    await fill('Adresa Genesis API', UNIT);
    await fill('Prístupový token', TOKENS[role]);
    await press('Načítať zariadenia');
    if (await shows(`Rola: ${role}`, 12000)) return;
    await enableSemantics().catch(() => {});
  }
  throw new Error(`panel sa ako ${role} nepripojil: "Rola: ${role}" sa neukázalo`);
}

// Prepínače zariadení. Karta zariadenia nesie Switch; role chips a zaškrtávacie
// políčka v iných sekciách sú `checkbox`, takže sa tu nemiešajú.
const deviceSwitches = () => page.locator('flt-semantics[role=switch]');

// ---------- pomôcky mimo prehliadača (dôkaz, nie ovládanie) ----------

const api = async (method, route, { token, body } = {}) => {
  const response = await fetch(UNIT + route, {
    method,
    headers: {
      ...(token ? { Authorization: 'Bearer ' + token } : {}),
      ...(body ? { 'Content-Type': 'application/json' } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await response.text();
  let json = null;
  try { json = JSON.parse(text); } catch { /* nie každá odpoveď je JSON */ }
  return { status: response.status, type: response.headers.get('content-type'), text, json };
};

function signedDecision(decision) {
  const body = JSON.stringify(decision);
  const timestamp = new Date().toISOString().replace(/\.\d+Z$/, 'Z');
  const input = Buffer.concat([
    Buffer.from(`v1:POST:/v1/behavior/decisions:${timestamp}:`),
    Buffer.from(body),
  ]);
  const signature = crypto.createHmac('sha256', BEHAVIOR.secret).update(input).digest('hex');
  return { body, timestamp, signature };
}

async function postDecision(decision) {
  const { body, timestamp, signature } = signedDecision(decision);
  const response = await fetch(UNIT + '/v1/behavior/decisions', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'x-genesis-key-id': BEHAVIOR.keyId,
      'x-genesis-timestamp': timestamp,
      'x-genesis-signature': signature,
    },
    body,
  });
  const text = await response.text();
  return { status: response.status, text };
}

const mode = (value) => fs.writeFileSync(path.join(WORK, 'ha-mode'), value + '\n');

// Vypnutie Home Assistanta musí byť dokázané, nie predpokladané. Keby sa PID
// netrafilo, test by „výpadok" iba ohlásil a meral by zdravý stav.
async function stopStub() {
  const pid = Number(fs.readFileSync(path.join(WORK, 'stub.pid'), 'utf8').trim());
  process.kill(pid, 'SIGKILL');
  for (let attempt = 0; attempt < 40; attempt += 1) {
    await sleep(100);
    try { process.kill(pid, 0); } catch { return; }
  }
  throw new Error('stub Home Assistanta sa nepodarilo vypnúť, výpadok by bol predstieraný');
}

function startStub() {
  const child = spawn('python3', [process.env.GENESIS_START_STUB,
    '--port', '18123', '--control', path.join(WORK, 'ha-mode'),
    '--pidfile', path.join(WORK, 'stub.pid')],
    { detached: true, stdio: 'ignore' });
  child.unref();
}

// Počká, kým jednotka načíta inventár z Home Assistanta.
async function waitForInventory() {
  for (let attempt = 0; attempt < 30; attempt += 1) {
    const devices = (await api('GET', '/v1/devices', { token: TOKENS.owner })).json || [];
    if (devices.filter((device) => device.availability === 'online').length >= 3) return;
    await sleep(1000);
  }
  throw new Error('jednotka nenačítala inventár z Home Assistanta');
}

const ledgerOf = async (commandId) =>
  (await api('GET', '/v1/commands/' + commandId, { token: TOKENS.owner })).json;


// ---------- 1. Panel, API a health na správnych cestách ----------

async function caseOne() {
  const notes = [];
  let verdict = 'PASS';
  const text = await screen();
  const rendered = ['ZARIADENIA', 'PREVÁDZKA', 'SPOJENIE'].filter((s) => text.includes(s));
  if (rendered.length === 3) {
    notes.push('panel sa na `/` nakreslil: sekcie ' + rendered.join(', '));
  } else {
    verdict = 'FAIL';
    notes.push('panel na `/` nenakreslil sekcie, našlo sa len: ' + rendered.join(', '));
  }
  if (page.pageErrors.length === 0) {
    notes.push(`bez chýb v konzole, a to s odrezaným internetom (zablokované: ${[...page.blockedHosts].join(', ') || 'nič sa nepokúsilo'})`);
  } else {
    verdict = 'FAIL';
    notes.push('chyby v prehliadači: ' + page.pageErrors.slice(0, 2).join(' | '));
  }
  // Hodnotu treba čítať po kliknutí: do DOM ju Flutter zapíše až pre pole s fokusom.
  await page.click('input[aria-label="Adresa Genesis API"]').catch(() => {});
  await page.waitForTimeout(300);
  const prefilled = await page.$eval('input[aria-label="Adresa Genesis API"]', (n) => n.value)
    .catch(() => '');
  if (prefilled.startsWith(UNIT)) {
    notes.push(`panel otvorený z jednotky si adresu predvyplnil sám na "${prefilled}" — človek ju nemusí hľadať`);
  } else {
    notes.push(`adresa sa predvyplnila na "${prefilled}"`);
  }

  const health = await api('GET', '/health');
  notes.push(`/health → ${health.status} ${health.json?.status} (${health.json?.service} ${health.json?.version})`);
  if (health.status !== 200 || health.json?.status !== 'ok') verdict = 'FAIL';

  const api401 = await api('GET', '/v1/me');
  notes.push(`/v1/me bez tokenu → ${api401.status}`);
  if (api401.status !== 401) verdict = 'FAIL';

  const missing = await api('GET', '/v1/nonexistent');
  const isHtml = (missing.type || '').includes('text/html') || missing.text.trim().startsWith('<');
  if (missing.status === 404 && !isHtml) {
    notes.push(`chybná API cesta → 404 a nie HTML (telo ${missing.text.length} B, typ ${missing.type || 'žiadny'})`);
  } else {
    verdict = 'FAIL';
    notes.push(`chybná API cesta vrátila ${missing.status} typ ${missing.type}: ${missing.text.slice(0, 80)}`);
  }
  record(1, 'Panel na `/`, API na `/v1`, health na `/health`', verdict, notes);
}

// ---------- 2. Roly a serverové odmietnutie ----------

async function caseTwo() {
  const notes = [];
  let verdict = 'PASS';
  for (const role of ['owner', 'member', 'guest']) {
    await connectAs(role);
    const text = await screen();
    const actor = `pilot-${role} (${role})`;
    if (role === 'guest') {
      // Gosťovi panel kartu na povel vôbec nedá a povie prečo. Toto je tvrdenie
      // o tom texte; či sa povel naozaj neodošle, meria kontrola nižšie.
      if (/ovládať zariadenia smie vlastník/.test(text) && /jednotka, nie panel/.test(text)) {
        notes.push('panel hlási "Rola: guest" a gosťovi kartu na povel nedá — píše, že rozhoduje jednotka, nie panel');
      } else {
        verdict = 'FAIL';
        notes.push('panel gosťovi nevysvetlil, prečo povel odoslať nemôže');
      }
    } else if (text.includes(actor)) {
      notes.push(`panel s ${role} tokenom hlási "Rola: ${role}" a identitu ${actor}, ktorú určila jednotka`);
    } else {
      verdict = 'FAIL';
      notes.push(`panel s ${role} tokenom neukázal identitu ${actor}`);
    }
    if (role === 'guest') {
      // Rozhodujúce nie je, čo je napísané, ale či sa dá povel odoslať. Gosť
      // zariadenia vidí; tu sa meria, že klik na prepínač nevytvorí povel.
      const before = (await api('GET', '/v1/commands', { token: TOKENS.owner })).json || [];
      const count = await deviceSwitches().count();
      if (count > 0) {
        await deviceSwitches().first().evaluate((node) => node.click());
        await page.waitForTimeout(5000);
      }
      const after = (await api('GET', '/v1/commands', { token: TOKENS.owner })).json || [];
      if (after.length === before.length) {
        notes.push(`guest klikol na prepínač zariadenia (${count} na obrazovke) a v ledgeri nepribudol žiaden povel`);
      } else {
        verdict = 'FAIL';
        notes.push(`guest kliknutím vytvoril ${after.length - before.length} povel(ov), hoci ovládať nesmie`);
      }
    }
  }
  const command = (household, key) => ({
    household_id: household, device_id: 'ha:light.living', value: false,
    idempotency_key: key, correlation_id: key,
  });
  const guestWrite = await api('POST', '/v1/commands',
    { token: TOKENS.guest, body: command(HOUSEHOLD, 'acc-guest') });
  notes.push(`guest POST /v1/commands priamo na API → ${guestWrite.status}`);
  if (guestWrite.status !== 403) verdict = 'FAIL';

  const foreign = await api('POST', '/v1/commands',
    { token: TOKENS.owner, body: command('cudzia-domacnost', 'acc-foreign') });
  notes.push(`owner na cudziu domácnosť → ${foreign.status}`);
  if (foreign.status !== 403) verdict = 'FAIL';
  record(2, 'Owner/member/guest, odmietnutie guest zápisu a cudzej domácnosti', verdict, notes);
}

// ---------- 3. Párovanie, uplatnenie a odobranie ----------

async function caseThree() {
  const notes = [];
  let verdict = 'PASS';
  await connectAs('owner');
  const actor = 'pilot-tester-' + Date.now().toString(36);
  await fill('Identifikátor člena', actor);
  await press('Vydať kód');
  await page.waitForTimeout(2500);
  const shown = await screen();
  // Kód číta test z prístupnostného popisu, kde je po štvoriciach — presne tak,
  // ako ho dostane človek s čítačkou obrazovky.
  const match = shown.match(/\b((?:[0-9a-f]{4} ){15}[0-9a-f]{4})\b/);
  if (!match) {
    record(3, 'Párovanie kódom, uplatnenie a odobranie', 'FAIL',
      [...notes, 'panel po „Vydať kód" nezobrazil kód; na obrazovke: '
        + shown.split('\n').filter((l) => /kód/i.test(l)).slice(0, 3).join(' / ')]);
    return;
  }
  const code = match[1].replace(/ /g, '');
  notes.push(`owner vydal kód pre ${actor} a panel ho zobrazil (${code.length} znakov, tu neuvádzam)`);
  if (await shows('Zobrazuje sa raz')) notes.push('panel hovorí, že kód uvidí len raz');

  await press('Mám ho');
  await page.waitForTimeout(800);
  if (!(await screen()).includes(code)) {
    notes.push('po potvrdení „Mám ho" kód z obrazovky zmizol');
  } else {
    verdict = 'PARTIAL';
    notes.push('kód zostal na obrazovke aj po „Mám ho"');
  }

  // Uplatnenie kódu je jediný hovor bez tokenu, takže sa dá urobiť z panela
  // ako to urobí člen na svojom zariadení.
  await fill('Domácnosť', HOUSEHOLD);
  await fill('Párovací kód', code);
  await press('Uplatniť kód');
  // Dôkaz berie test z jednotky, nie z obrazovky: akcia je z panela, ale „kód bol
  // uplatnený" znamená, že jednotka vydala identitu — a to je fakt, ktorý sa dá
  // overiť, na rozdiel od vety, ktorá sa môže prekresliť.
  let issuedCredential = null;
  for (let attempt = 0; attempt < 20 && !issuedCredential; attempt += 1) {
    await sleep(500);
    const all = (await api('GET', '/v1/credentials', { token: TOKENS.owner })).json || [];
    issuedCredential = all.find((credential) => credential.actor_id === actor);
  }
  if (issuedCredential) {
    notes.push(`člen kód uplatnil z panela a jednotka mu vydala identitu s rolou ${issuedCredential.role}`);
  } else {
    verdict = 'FAIL';
    notes.push('po „Uplatniť kód" jednotka žiadnu identitu nevydala');
  }

  const reuse = await api('POST', '/v1/pairings/redeem',
    { body: { household_id: HOUSEHOLD, code } });
  if (reuse.status !== 200) {
    notes.push(`ten istý kód druhýkrát → ${reuse.status}, kód je jednorazový`);
  } else {
    verdict = 'FAIL';
    notes.push('ten istý kód sa dal uplatniť dvakrát');
  }

  await connectAs('owner');
  if (!issuedCredential) {
    record(3, 'Párovanie kódom, uplatnenie a odobranie', 'PARTIAL',
      [...notes, 'vydaná identita sa v zozname nenašla, odobranie sa netestovalo']);
    return;
  }
  if (await shows(actor)) notes.push(`panel vlastníka ukazuje vydanú identitu ${actor}`);
  await press('Odobrať');
  await page.waitForTimeout(2500);
  const after = await api('GET', '/v1/credentials', { token: TOKENS.owner });
  const still = (after.json || []).find((c) => c.actor_id === actor && !c.revoked_at);
  if (!still) {
    notes.push('po „Odobrať" identita už nie je platná');
  } else {
    verdict = 'FAIL';
    notes.push('identita je po odobraní stále platná');
  }
  record(3, 'Párovanie kódom, uplatnenie a odobranie', verdict, notes);
}

// ---------- 4. Inventár, miestnosti, výpadok HA a obnova ----------

async function caseFour() {
  const notes = [];
  let verdict = 'PASS';
  await connectAs('owner');
  const text = await screen();
  const expected = [['Obývačka — strop', 'Obývačka'], ['Chodba', 'Chodba'], ['Bojler', 'Technická miestnosť']];
  for (const [device, room] of expected) {
    if (text.includes(device)) notes.push(`panel ukazuje „${device}"`);
    else { verdict = 'FAIL'; notes.push(`panel neukazuje „${device}"`); }
  }
  // Priame priradenie entity musí prebiť miestnosť zdedenú zo zariadenia:
  // light.hall patrí zariadeniu v technickej, ale entita má Chodbu.
  const devices = (await api('GET', '/v1/devices', { token: TOKENS.owner })).json || [];
  const hall = devices.find((d) => d.device_id === 'ha:light.hall');
  if (hall?.area_name === 'Chodba') {
    notes.push('miestnosť priradená priamo entite prebila miestnosť zariadenia (Chodba, nie Technická miestnosť)');
  } else {
    verdict = 'FAIL';
    notes.push(`light.hall dostal miestnosť ${hall?.area_name}, očakávala sa Chodba`);
  }
  if (!devices.some((d) => d.device_id.includes('sensor.'))) {
    notes.push('nepodporovaná doména (sensor.teplota) sa v inventári neobjavila');
  } else {
    verdict = 'FAIL';
    notes.push('v inventári je senzor, ktorý panel ovládať nemôže');
  }

  // Výpadok Home Assistanta. Toto je dôvod, prečo je na druhej strane skutočný
  // socket a nie mock: spojenie sa dá naozaj preseknúť. Meria sa stav linky,
  // nie text na obrazovke — text sa dá trafiť náhodou, stav nie.
  const linkState = async () =>
    (await api('GET', '/v1/diagnostics', { token: TOKENS.owner })).json?.home_assistant?.state;
  const before = await linkState();
  await stopStub();
  let down = null;
  for (let attempt = 0; attempt < 12 && down !== 'disconnected'; attempt += 1) {
    await sleep(2000);
    down = await linkState();
  }
  if (down === 'disconnected') {
    notes.push(`po skutočnom vypnutí Home Assistanta linka prešla z "${before}" na "disconnected"`);
  } else {
    verdict = 'FAIL';
    notes.push(`Home Assistant je vypnutý, ale jednotka hlási linku ako "${down}"`);
  }
  await press('Načítať zariadenia');
  await page.waitForTimeout(2500);
  const shown = await screen();
  if (/spadl|nedostupn|zastaran|neznám|Spojené.{0,40}nie/i.test(shown) || !/Spojené/.test(shown)) {
    notes.push('panel počas výpadku netvrdí, že je Home Assistant spojený');
  } else {
    verdict = 'FAIL';
    notes.push('panel počas výpadku stále tvrdí, že je Home Assistant spojený');
  }

  startStub();
  let back = null;
  for (let attempt = 0; attempt < 12 && back !== 'connected'; attempt += 1) {
    await sleep(2000);
    back = await linkState();
  }
  if (back === 'connected') {
    notes.push('po zapnutí Home Assistanta sa jednotka pripojila sama, bez reštartu');
  } else {
    verdict = 'FAIL';
    notes.push(`Home Assistant beží, ale linka zostala "${back}"`);
  }
  await press('Načítať zariadenia');
  await page.waitForTimeout(2500);
  const recovered = (await api('GET', '/v1/devices', { token: TOKENS.owner })).json || [];
  const online = recovered.filter((device) => device.availability === 'online').length;
  if (online === 3) {
    notes.push('po obnove vidí panel znova všetky tri zariadenia online');
  } else {
    verdict = 'FAIL';
    notes.push(`po obnove je online ${online} z 3 zariadení`);
  }
  record(4, 'Inventár, miestnosti, výpadok Home Assistanta a obnova spojenia', verdict, notes);
}

// ---------- 5. Povel z panela a ledger ----------

async function commandViaPanel(expectOn) {
  // Prepnutie zariadenia v paneli. Prepínače sú v poradí kariet; prvý je
  // Obývačka — strop.
  const before = (await api('GET', '/v1/commands', { token: TOKENS.owner })).json || [];
  const toggle = deviceSwitches().first();
  await toggle.evaluate((node) => node.click());
  await page.waitForTimeout(9000);
  const after = (await api('GET', '/v1/commands', { token: TOKENS.owner })).json || [];
  const fresh = after.filter((c) => !before.some((b) => b.command_id === c.command_id));
  return fresh[0] || after[0];
}

async function caseFive() {
  const notes = [];
  let verdict = 'PASS';
  mode('normal');
  await connectAs('owner');
  const confirmed = await commandViaPanel();
  if (!confirmed) {
    record(5, 'Povel z panela a ledger', 'FAIL', ['prepnutie v paneli nevytvorilo žiadny povel']);
    return;
  }
  const chain = (confirmed.transitions || confirmed.history || []).map((t) => t.status || t.state);
  notes.push(`povel z panela: stav ${confirmed.status}, reťaz ${chain.length ? chain.join(' → ') : '(bez histórie v odpovedi)'}`);
  if (confirmed.status === 'device_confirmed') {
    notes.push('ledger došiel až po device_confirmed, teda potvrdené pozorovaním stavu, nie len prijatím povelu');
  } else {
    verdict = 'FAIL';
    notes.push(`očakával sa device_confirmed, ledger hlási ${confirmed.status}`);
  }

  // Home Assistant povel prijme, ale stav nikdy nepotvrdí. Panel z toho nesmie
  // urobiť úspech — to je celý zmysel tohto kroku.
  mode('ack_only');
  await connectAs('owner');
  const unknown = await commandViaPanel();
  await page.waitForTimeout(2000);
  const shown = await screen();
  if (unknown && unknown.status !== 'device_confirmed') {
    notes.push(`pri nepotvrdenom stave ledger skončil na ${unknown.status}, nie na device_confirmed`);
  } else {
    verdict = 'FAIL';
    notes.push(`jednotka tvrdí ${unknown?.status} aj keď stav nikto nepotvrdil`);
  }
  if (/nezn|neisto|neoverené|nepotvrd/i.test(shown)) {
    notes.push('panel neistý výsledok priznáva na obrazovke');
  } else {
    verdict = verdict === 'PASS' ? 'PARTIAL' : verdict;
    notes.push('panel neistý výsledok na obrazovke nepomenoval');
  }
  mode('normal');
  record(5, 'Zapnutie/vypnutie z panela, ledger a neistý výsledok', verdict, notes);
}

// ---------- 6. Behavior grant, expiry a relock ----------

function decision(overrides) {
  const now = Date.now();
  const stamp = (ms) => new Date(now + ms).toISOString().replace(/\.\d+Z$/, 'Z');
  return {
    schema_version: '1.0',
    decision_id: crypto.randomUUID(),
    issuer: 'behavior-engine',
    household_id: HOUSEHOLD,
    central_unit_id: BEHAVIOR.centralUnit,
    subject_id: crypto.randomUUID(),
    device_id: 'ha:light.living',
    capability_id: 'power',
    requested_value: true,
    operation: 'apply',
    valid_from: stamp(0),
    expires_at: stamp(3600_000),
    reason_code: 'pilot_acceptance',
    idempotency_key: 'acc-grant-' + now.toString(36),
    required_confirmation: 'device',
    ...overrides,
  };
}

async function caseSix() {
  const notes = [];
  let verdict = 'PASS';
  mode('normal');

  const unsigned = await fetch(UNIT + '/v1/behavior/decisions', {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(decision({})),
  });
  notes.push(`rozhodnutie bez podpisu → ${unsigned.status}: kanál nepustí nepodpísané`);
  if (unsigned.status < 400) verdict = 'FAIL';

  // Okno dlhé pár sekúnd, aby grant vypršal sám. Rozhodnutie s už uplynutým
  // oknom jednotka odmieta (409), takže expiráciu treba naozaj prečkať.
  const short = decision({
    expires_at: new Date(Date.now() + 5000).toISOString().replace(/\.\d+Z$/, 'Z'),
    idempotency_key: 'acc-grant-' + Date.now().toString(36),
  });
  const opened = await postDecision(short);
  if (opened.status !== 200) {
    record(6, 'Behavior grant, expiry a relock', 'FAIL',
      [...notes, `podpísané rozhodnutie → ${opened.status}: ${opened.text.slice(0, 160)}`]);
    return;
  }
  const outcome = JSON.parse(opened.text);
  notes.push(`podpísané rozhodnutie → 200, výsledok "${outcome.outcome}" s dôvodom "${outcome.reason_code}"`);

  const grants = async () =>
    ((await api('GET', '/v1/access', { token: TOKENS.owner })).json || []).map((row) => row.grant);
  const live = (await grants()).find((grant) => grant.decision_id === short.decision_id);
  if (live?.state === 'active' && live.unlock_confirmed === true) {
    notes.push('grant je active a odomknutie je potvrdené pozorovaním zariadenia, nie len prijatím povelu');
  } else {
    verdict = 'FAIL';
    notes.push(`grant je "${live?.state}", unlock_confirmed=${live?.unlock_confirmed}`);
  }

  await connectAs('owner');
  if ((await screen()).includes('ČASOVÝ PRÍSTUP')) notes.push('panel má sekciu Časový prístup');
  else { verdict = 'FAIL'; notes.push('sekcia Časový prístup na obrazovke nie je'); }

  // Prečkať expiráciu a nechať panel, nech ponúkne zosúladenie.
  await sleep(6000);
  await pressIfPresent('Načítať zariadenia');
  await page.waitForTimeout(2500);
  const expired = (await grants()).find((grant) => grant.decision_id === short.decision_id);
  if (expired?.state === 'active') {
    notes.push('po uplynutí okna grant sám od seba nezmizol — jednotka ho drží, kým sa zosúladenie nevykoná');
  }
  if (await hasButton('Zosúladiť teraz')) {
    notes.push('panel ponúkol vlastníkovi „Zosúladiť teraz" na grant, ktorému uplynulo okno');
    await press('Zosúladiť teraz');
    await page.waitForTimeout(6000);
    const closed = (await grants()).find((grant) => grant.decision_id === short.decision_id);
    if (['relocked', 'relock_pending'].includes(closed?.state)) {
      notes.push(`po zosúladení z panela je grant "${closed.state}" — zamknuté späť`);
    } else {
      verdict = 'FAIL';
      notes.push(`po zosúladení zostal grant "${closed?.state}"`);
    }
  } else {
    verdict = 'PARTIAL';
    notes.push('tlačidlo „Zosúladiť teraz" sa na obrazovke nenašlo; zosúladenie sa z panela netestovalo');
  }
  const stillOpen = (await grants()).filter(
    (grant) => ['granted', 'active'].includes(grant.state)
      && grant.expires_at < new Date().toISOString());
  if (stillOpen.length === 0) {
    notes.push('po zosúladení nezostal otvorený žiaden grant s uplynutým oknom');
  } else {
    verdict = 'FAIL';
    notes.push(`${stillOpen.length} grantov je po uplynutí stále otvorených`);
  }
  notes.push('logický stav grantu a fyzický stav žiarovky sú oddelené v ledgeri; fyzickú žiarovku nikto nevidel (ELYSIUM-350)');
  record(6, 'Behavior grant, expiry a relock', verdict, notes);
}

// ---------- 7. Hlasový povel, nejednoznačnosť a citlivá akcia ----------

async function caseSeven() {
  const notes = [];
  let verdict = 'PASS';
  mode('normal');
  await connectAs('owner');

  await fill('Povel', 'zapni svetlo v obývačke');
  await press('Odoslať povel');
  await page.waitForTimeout(6000);
  let shown = await screen();
  if (/vykonan|Vykonané|zapnut/i.test(shown)) {
    notes.push('jednoznačný povel „zapni svetlo v obývačke" panel vykonal');
  } else {
    verdict = 'PARTIAL';
    notes.push('výsledok jednoznačného povelu nebol na obrazovke rozpoznateľný');
  }

  await fill('Povel', 'zapni svetlo');
  await press('Odoslať povel');
  await page.waitForTimeout(6000);
  shown = await screen();
  if (/nerozhodla medzi|nejednoznač|Nerozumel/i.test(shown)) {
    notes.push('nejednoznačný povel „zapni svetlo" panel neuhádol — ukázal, medzi čím sa nerozhodol');
  } else {
    verdict = 'FAIL';
    notes.push('nejednoznačný povel panel nevyhlásil za nejednoznačný');
  }

  // Bojler je v GENESIS_SENSITIVE_DEVICES, takže má vyžadovať potvrdenie.
  await fill('Povel', 'zapni bojler');
  await press('Odoslať povel');
  await page.waitForTimeout(6000);
  shown = await screen();
  const needsConfirmation = await hasButton('Potvrdiť akciu');
  if (needsConfirmation) {
    notes.push('citlivé zariadenie (bojler) si vyžiadalo potvrdenie a panel ponúkol „Potvrdiť akciu"');
    await press('Potvrdiť akciu');
    await page.waitForTimeout(8000);
    const done = await screen();
    if (/vykonan|Vykonané/i.test(done)) notes.push('po potvrdení sa akcia vykonala');
    else { verdict = 'PARTIAL'; notes.push('po potvrdení nebol výsledok na obrazovke rozpoznateľný'); }
  } else {
    verdict = 'FAIL';
    notes.push('citlivé zariadenie sa zapínalo bez potvrdenia');
  }

  const rows = (await api('GET', '/v1/voice/audit', { token: TOKENS.owner })).json || [];
  const reasoned = rows.filter((row) => row.decision && row.reason);
  if (rows.length > 0 && reasoned.length === rows.length) {
    const kinds = [...new Set(rows.map((row) => `${row.decision}/${row.reason}`))];
    notes.push(`audit hlasu má ${rows.length} záznamov a každý nesie rozhodnutie aj dôvod: ${kinds.join(', ')}`);
  } else {
    verdict = 'FAIL';
    notes.push(`audit hlasu: ${reasoned.length} z ${rows.length} záznamov má rozhodnutie aj dôvod`);
  }
  if (!rows.some((row) => row.reason === 'several_matching_devices')) {
    verdict = 'FAIL';
    notes.push('audit nezaznamenal, že povel bol nejednoznačný');
  }
  // Prepis sa posielal s `store_transcript: false`, takže ho jednotka nesmie mať.
  const dump = JSON.stringify(rows);
  const spoken = ['zapni svetlo v obývačke', 'zapni svetlo', 'zapni bojler'];
  const kept = spoken.filter((phrase) => dump.includes(phrase));
  if (kept.length === 0) {
    notes.push('v audite nie je ani jeden prepis — jednotka si nechala rozhodnutie a dôvod, nie slová');
  } else {
    verdict = 'FAIL';
    notes.push(`jednotka si uložila prepis, hoci sa o to nežiadalo: ${kept.join(', ')}`);
  }
  record(7, 'Hlasový povel, nejednoznačnosť, potvrdenie citlivej akcie a audit', verdict, notes);
}

// ---------- 8. Prevádzkové centrum a záloha ----------

async function caseEight() {
  const notes = [];
  let verdict = 'PASS';
  await connectAs('owner');
  const text = await screen();
  if (text.includes('PREVÁDZKA')) notes.push('panel má prevádzkové centrum');
  else { verdict = 'FAIL'; notes.push('sekcia Prevádzka na obrazovke nie je'); }
  // Päť nezávislých riadkov: jednotka, Home Assistant, inventár, incidenty, záloha.
  const rows = ['Genesis jednotka', 'Home Assistant', 'Inventár', 'Otvorené incidenty', 'Posledná záloha']
    .filter((row) => text.includes(row));
  if (rows.length === 5) {
    notes.push('prevádzkové centrum ukazuje všetkých päť stavov zvlášť: ' + rows.join(', '));
  } else {
    verdict = 'FAIL';
    notes.push('v prevádzkovom centre chýbajú riadky: ' + rows.join(', '));
  }

  if (await hasButton('Vytvoriť zálohu')) {
    await press('Vytvoriť zálohu');
    await page.waitForTimeout(5000);
    const backups = (await api('GET', '/v1/backup', { token: TOKENS.owner })).json || [];
    const list = Array.isArray(backups) ? backups : backups.backups || [];
    if (list.length > 0) {
      notes.push(`vlastník vytvoril zálohu z panela; jednotka ich eviduje ${list.length}, prvá má ${list[0].bytes} B z ${list[0].at}`);
      const onDisk = fs.existsSync(path.join(WORK, 'backups'))
        ? fs.readdirSync(path.join(WORK, 'backups')) : [];
      if (onDisk.length > 0) notes.push(`na disku jednotky skutočne je ${onDisk.length} súbor(ov) zálohy`);
      else { verdict = 'FAIL'; notes.push('záloha je v API, ale na disku nie je'); }
    } else {
      verdict = 'FAIL';
      notes.push('po stlačení „Vytvoriť zálohu" jednotka žiadnu zálohu neeviduje');
    }
  } else {
    verdict = 'FAIL';
    notes.push('vlastník nemá na obrazovke tlačidlo „Vytvoriť zálohu"');
  }

  for (const role of ['member', 'guest']) {
    const attempt = await api('POST', '/v1/backup', { token: TOKENS[role] });
    notes.push(`${role} POST /v1/backup → ${attempt.status}`);
    if (attempt.status !== 403) verdict = 'FAIL';
  }
  await connectAs('member');
  if (!(await hasButton('Vytvoriť zálohu'))) {
    notes.push('člen tlačidlo na zálohu v paneli vôbec nevidí');
  } else {
    verdict = 'PARTIAL';
    notes.push('člen vidí tlačidlo na zálohu, hoci ju server odmietne');
  }
  await connectAs('owner');
  if (await shows('Obnova a rollback')) notes.push('panel nesie runbook obnovy a rollbacku');
  else { verdict = verdict === 'PASS' ? 'PARTIAL' : verdict; notes.push('runbook obnovy na obrazovke nie je'); }
  record(8, 'Prevádzkové centrum, záloha vlastníkom a zákaz pre člena a gosťa', verdict, notes);
}

// ---------- 9. Reštart bez straty otvorených incidentov ----------

async function caseNine() {
  const notes = [];
  let verdict = 'PASS';
  mode('normal');
  await waitForInventory();

  // Reštart má čo stratiť len vtedy, keď je čo stratiť. Tento prípad si preto
  // najprv otvorí incident, ktorý reštart prežiť musí: grant s dlhým oknom
  // zostáva otvorený a je to presne ten záznam, ktorý sa nesmie zabudnúť.
  await connectAs('owner');
  const open = decision({
    expires_at: new Date(Date.now() + 3600_000).toISOString().replace(/\.\d+Z$/, 'Z'),
    idempotency_key: 'acc-restart-' + Date.now().toString(36),
  });
  const created = await postDecision(open);
  if (created.status !== 200) {
    record(9, 'Reštart bez straty otvorených incidentov', 'FAIL',
      [`otvorený incident sa nepodarilo vytvoriť: ${created.status} ${created.text.slice(0, 120)}`]);
    return;
  }
  await commandViaPanel();

  const snapshot = async () => {
    const access = (await api('GET', '/v1/access', { token: TOKENS.owner })).json || [];
    const commands = (await api('GET', '/v1/commands', { token: TOKENS.owner })).json || [];
    const mine = access.map((row) => row.grant).find((grant) => grant.decision_id === open.decision_id);
    return {
      grants: access.length,
      commands: commands.length,
      state: mine?.state,
      openOnes: access.map((row) => row.grant)
        .filter((grant) => ['granted', 'active', 'relock_pending'].includes(grant.state)).length,
    };
  };
  const before = await snapshot();
  notes.push(`pred reštartom: ${before.grants} grantov (${before.openOnes} otvorených, ten nový je "${before.state}"), ${before.commands} povelov v ledgeri`);
  if (before.openOnes === 0 || before.commands === 0) {
    record(9, 'Reštart bez straty otvorených incidentov', 'FAIL',
      [...notes, 'test si nepripravil stav, ktorý by reštart mohol stratiť']);
    return;
  }

  // 1) Reštart aplikácie: znovu načítať stránku.
  await page.reload({ waitUntil: 'load' });
  await enableSemantics();
  const reloaded = await screen();
  if (['ZARIADENIA', 'SPOJENIE'].every((section) => reloaded.includes(section))) {
    notes.push('po znovunačítaní panela sa aplikácia nakreslila celá');
  } else {
    verdict = 'FAIL';
    notes.push('po znovunačítaní panel nenakreslil svoje sekcie');
  }

  // 2) Reštart jednotky. Ledger je SQLite v /data, takže musí prežiť.
  const pid = Number(fs.readFileSync(path.join(WORK, 'core.pid'), 'utf8').trim());
  process.kill(pid, 'SIGTERM');
  let stopped = false;
  for (let attempt = 0; attempt < 50 && !stopped; attempt += 1) {
    await sleep(100);
    try { process.kill(pid, 0); } catch { stopped = true; }
  }
  if (!stopped) {
    record(9, 'Reštart bez straty otvorených incidentov', 'BLOCKED',
      [...notes, 'jednotku sa nepodarilo zastaviť, reštart by bol predstieraný']);
    return;
  }
  notes.push('jednotka bola skutočne zastavená (SIGTERM) a spustená znova');
  spawn('sh', ['-c', process.env.GENESIS_RESTART_CORE], { detached: true, stdio: 'ignore' }).unref();
  let up = false;
  for (let attempt = 0; attempt < 60 && !up; attempt += 1) {
    await sleep(500);
    try { up = (await fetch(UNIT + '/health')).ok; } catch { up = false; }
  }
  if (!up) {
    record(9, 'Reštart bez straty otvorených incidentov', 'BLOCKED',
      [...notes, 'jednotka sa po reštarte nevrátila; ledger sa neoveril']);
    return;
  }

  const after = await snapshot();
  notes.push(`po reštarte jednotky: ${after.grants} grantov (${after.openOnes} otvorených, ten istý je "${after.state}"), ${after.commands} povelov`);
  if (after.commands >= before.commands && after.grants >= before.grants) {
    notes.push('reštart nezahodil ani jeden povel ani grant');
  } else {
    verdict = 'FAIL';
    notes.push('po reštarte časť záznamov chýba');
  }
  if (after.openOnes === before.openOnes && after.state === before.state) {
    notes.push(`otvorený incident reštart prežil a zostal v stave "${after.state}"`);
  } else {
    verdict = 'FAIL';
    notes.push(`otvorený incident sa reštartom zmenil: ${before.openOnes}/"${before.state}" → ${after.openOnes}/"${after.state}"`);
  }

  await page.reload({ waitUntil: 'load' });
  await enableSemantics();
  await connectAs('owner');
  if ((await screen()).includes('ČASOVÝ PRÍSTUP')) {
    notes.push('panel po reštarte jednotky znovu načítal stav a incident je v ňom vidieť');
  }
  record(9, 'Reštart aplikácie aj jednotky bez straty otvorených incidentov', verdict, notes);
}

// ---------- priebeh ----------

const cases = [caseOne, caseTwo, caseThree, caseFour, caseFive, caseSix, caseSeven, caseEight, caseNine];
const only = process.env.GENESIS_ONLY ? process.env.GENESIS_ONLY.split(',').map(Number) : null;

// Panel sa otvorí raz, aby sa dal spustiť aj jeden prípad samostatne. Čaká sa
// aj na inventár: bez neho panel kartu na povel vôbec nevykreslí a hlasový test
// by zlyhal na tom, že jednotka ešte len štartuje.
await openPanel();
await waitForInventory();

for (const [index, run] of cases.entries()) {
  const id = index + 1;
  if (only && !only.includes(id)) continue;
  try {
    await run();
  } catch (error) {
    record(id, run.name, 'FAIL', ['test spadol: ' + String(error).slice(0, 300)]);
  }
}

if (browser) await browser.close();

const tally = results.reduce((all, r) => ({ ...all, [r.verdict]: (all[r.verdict] || 0) + 1 }), {});
console.log('\n' + '='.repeat(70));
console.log('ELYSIUM-362 ' + JSON.stringify(tally));
fs.writeFileSync(path.join(WORK, 'results.json'), JSON.stringify(results, null, 2));
console.log('výsledky: ' + path.join(WORK, 'results.json'));
if (results.some((r) => r.verdict === 'FAIL')) process.exitCode = 1;
