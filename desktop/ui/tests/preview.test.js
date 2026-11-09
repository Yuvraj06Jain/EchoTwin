// Automated "no bugs" pass for desktop/ui (Chromium via Playwright).
const { chromium } = require('playwright');
const fs = require('fs');
const path = require('path');
const jsQR = require('jsqr');
const { PNG } = require('pngjs');

const URL = 'file://' + path.resolve(__dirname, '..', 'index.html');
const OUT = path.join(__dirname, 'shots');
fs.mkdirSync(OUT, { recursive: true });

let failures = 0;
let passes = 0;
function check(cond, what) {
  if (cond) passes++;
  else {
    failures++;
    console.log(`  FAIL: ${what}`);
  }
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function newPage(browser, viewport = { width: 1280, height: 860 }, scale = 1) {
  const page = await browser.newPage({ viewport, deviceScaleFactor: scale });
  page.problems = [];
  page.on('console', (m) => {
    if (['error', 'warning'].includes(m.type())) page.problems.push(`${m.type()}: ${m.text()}`);
  });
  page.on('pageerror', (e) => page.problems.push(`pageerror: ${e.message}`));
  page.on('dialog', (d) => {
    page.problems.push(`unexpected dialog: ${d.message()}`);
    d.dismiss();
  });
  await page.goto(URL);
  await page.waitForSelector('.notif');
  return page;
}

const text = (page, sel) => page.$eval(sel, (e) => e.textContent.trim());
const count = (page, sel) => page.$$eval(sel, (els) => els.filter((e) => e.offsetParent !== null).length);
const toastText = (page) => page.$eval('#toast', (e) => (e.hidden ? '' : e.textContent));
const focusedInside = (page, sel) => page.evaluate((s) => !!document.querySelector(s)?.contains(document.activeElement), sel);

// Every visible element must fit inside its clipping ancestor, and text must not spill out
// of boxes that don't intentionally ellipsize.
async function layoutProblems(page, label) {
  return page.evaluate((label) => {
    const issues = [];
    const win = document.getElementById('window');
    const wr = win.getBoundingClientRect();
    if (win.scrollWidth > win.clientWidth + 1) issues.push(`${label}: window scrolls horizontally`);
    for (const el of win.querySelectorAll('*')) {
      if (el.offsetParent === null || el.closest('svg')) continue;
      const cs = getComputedStyle(el);
      const r = el.getBoundingClientRect();
      if (r.width === 0 || r.height === 0) continue;
      let clip = el.parentElement;
      while (clip && clip !== win && getComputedStyle(clip).overflowX === 'visible') clip = clip.parentElement;
      const cr = (clip || win).getBoundingClientRect();
      if (r.right > cr.right + 1 || r.left < cr.left - 1) {
        issues.push(`${label}: <${el.tagName.toLowerCase()} class="${el.className}"> sticks out horizontally (${Math.round(r.left)}..${Math.round(r.right)} vs ${Math.round(cr.left)}..${Math.round(cr.right)})`);
      }
      if (cs.overflowX === 'visible' && cs.textOverflow !== 'ellipsis' && el.scrollWidth > el.clientWidth + 1 && el.children.length === 0 && el.clientWidth > 0) {
        issues.push(`${label}: text overflows <${el.tagName.toLowerCase()} class="${el.className}"> "${el.textContent.slice(0, 40)}"`);
      }
    }
    if (wr.right > window.innerWidth + 1) issues.push(`${label}: window wider than viewport`);
    return issues;
  }, label);
}

async function auditAllScreens(page, size) {
  const all = [];
  for (const mode of ['connected', 'offline']) {
    await page.click(`[data-preview="${mode}"]`);
    await page.waitForTimeout(350);
    for (const tab of ['notifications', 'messages', 'files']) {
      await page.click(`#tab-${tab}`);
      await page.waitForTimeout(120);
      all.push(...(await layoutProblems(page, `${size} ${mode}/${tab}`)));
    }
    await page.click('[data-action="open-settings"]');
    await page.waitForTimeout(220);
    all.push(...(await layoutProblems(page, `${size} ${mode}/settings`)));
    await page.keyboard.press('Escape');
  }
  await page.click('[data-preview="unpaired"]');
  await page.waitForTimeout(350);
  all.push(...(await layoutProblems(page, `${size} pairing`)));
  await page.click('[data-preview="connected"]');
  await page.waitForTimeout(350);
  await page.click('#tab-notifications');
  return all;
}

(async () => {
  const browser = await chromium.launch();

  // ---------------------------------------------------------------- layout at two sizes
  console.log('Layout');
  for (const [vp, size] of [[{ width: 1280, height: 860 }, '1180x760'], [{ width: 948, height: 680 }, '900x600']]) {
    const page = await newPage(browser, vp);
    const dims = await page.$eval('#window', (w) => [Math.round(w.getBoundingClientRect().width), Math.round(w.getBoundingClientRect().height)]);
    console.log(`  window rendered at ${dims[0]}x${dims[1]} for target ${size}`);
    const issues = await auditAllScreens(page, size);
    issues.forEach((i) => console.log(`  - ${i}`));
    check(issues.length === 0, `no layout overflow at ${size}`);
    check(page.problems.length === 0, `no console problems at ${size}: ${page.problems.join(' | ')}`);
    const radius = await page.$eval('#window', (w) => getComputedStyle(w).borderTopLeftRadius);
    check(radius === '12px', `window has rounded corners (got ${radius})`);
    await page.close();
  }

  const page = await newPage(browser);

  // ---------------------------------------------------------------- title bar
  console.log('Title bar');
  check((await page.$$eval('.titlebar button svg', (s) => s.length)) === 4, 'all four title-bar buttons have icons');
  await page.click('[data-action="win-close"]');
  check((await toastText(page)).includes('desktop app'), 'window buttons explain themselves in the preview');

  // ---------------------------------------------------------------- tabs
  console.log('Tabs');
  await page.click('#tab-messages');
  check(await page.$eval('#panel-messages', (p) => !p.hidden), 'Messages panel visible after click');
  check((await page.getAttribute('#tab-messages', 'aria-selected')) === 'true', 'aria-selected follows the active tab');
  check(await page.$eval('#panel-notifications', (p) => p.hidden), 'other panels hidden');
  await page.focus('#tab-messages');
  await page.keyboard.press('ArrowRight');
  check(await page.$eval('#panel-files', (p) => !p.hidden), 'ArrowRight moves to Files');
  check(await page.evaluate(() => document.activeElement.id === 'tab-files'), 'focus follows arrow navigation');
  await page.keyboard.press('Home');
  check(await page.$eval('#panel-notifications', (p) => !p.hidden), 'Home goes to the first tab');

  // ---------------------------------------------------------------- notifications
  console.log('Notifications');
  const before = await count(page, '.notif');
  const badgeBefore = await text(page, '#tab-notifications .badge');
  check(String(before) === badgeBefore, `tab badge matches notification count (${before} vs ${badgeBefore})`);

  await page.click('.notif[data-id="n1"] [data-action="reply-open"]');
  check(await page.evaluate(() => document.activeElement.name === 'reply'), 'Reply opens a focused input');
  await page.keyboard.press('Enter');
  await page.waitForTimeout(400);
  check((await count(page, '.notif[data-id="n1"] .reply-form')) === 1, 'empty reply is not sent');
  await page.keyboard.press('Escape');
  check((await count(page, '.reply-form')) === 0, 'Esc closes the reply box');
  check(await page.evaluate(() => document.activeElement.dataset.action === 'reply-open'), 'focus returns to Reply after Esc');
  await page.click('.notif[data-id="n1"] [data-action="reply-open"]');
  await page.keyboard.type('On my way');
  await page.click('.notif[data-id="n1"] .reply-form [type="submit"]');
  await page.waitForTimeout(450);
  check((await toastText(page)) === 'Reply sent to Rahul', 'reply confirmation toast');
  check((await count(page, '.notif[data-id="n1"] .notif-note')) === 1, 'notification marked Replied');

  // draft survives re-render
  await page.click('.notif[data-id="n2"] [data-action="reply-open"]');
  await page.keyboard.type('draft text');
  await page.click('.notif[data-id="n5"] [data-action="reply-open"]');
  await page.click('.notif[data-id="n2"] [data-action="reply-open"]');
  check((await page.$eval('.notif[data-id="n2"] input', (i) => i.value)) === 'draft text', 'reply draft is kept when switching');
  await page.keyboard.press('Escape');

  // double-click dismiss removes exactly one
  await page.dblclick('.notif[data-id="n6"] [data-action="dismiss"]');
  await page.waitForTimeout(400);
  check((await count(page, '.notif')) === before - 1, 'double-clicking Dismiss removes exactly one notification');
  check((await text(page, '#tab-notifications .badge')) === String(before - 1), 'badge updates after dismiss');

  // keyboard dismiss moves focus to the next notification
  await page.focus('.notif[data-id="n3"] [data-action="dismiss"]');
  await page.keyboard.press('Enter');
  await page.waitForTimeout(400);
  check(await page.evaluate(() => document.activeElement.closest('.notif')?.dataset.id === 'n4'), 'focus moves to the next notification after dismiss');

  // ---------------------------------------------------------------- settings sheet
  console.log('Settings');
  await page.click('[data-action="open-settings"]');
  await page.waitForTimeout(200);
  check(await focusedInside(page, '.sheet'), 'focus moves into settings');
  let escaped = false;
  for (let i = 0; i < 25; i++) {
    await page.keyboard.press('Tab');
    if (!(await focusedInside(page, '.sheet'))) escaped = true;
  }
  for (let i = 0; i < 25; i++) {
    await page.keyboard.press('Shift+Tab');
    if (!(await focusedInside(page, '.sheet'))) escaped = true;
  }
  check(!escaped, 'Tab / Shift+Tab stay inside the settings sheet');

  const calBtn = '.sheet [data-action="toggle-app"][data-name="Calendar"]';
  await page.click(calBtn);
  await page.waitForTimeout(250);
  check((await page.getAttribute(calBtn, 'aria-checked')) === 'false', 'app toggle switches off');
  check(await page.evaluate((s) => document.activeElement === document.querySelector(s), calBtn), 'toggle keeps focus');
  await page.keyboard.press('Escape');
  check((await count(page, '.backdrop')) === 0, 'Esc closes settings');
  check(await page.evaluate(() => document.activeElement.dataset.action === 'open-settings'), 'focus returns to the settings button');
  check((await count(page, '.notif-group[aria-label="Calendar"]')) === 0, 'disabled app disappears from notifications');

  await page.click('[data-action="open-settings"]');
  await page.waitForSelector('.sheet');
  await page.mouse.click(120, 400); // outside the sheet, on the backdrop
  check((await count(page, '.backdrop')) === 0, 'clicking outside closes settings');

  // ---------------------------------------------------------------- clipboard toggle
  console.log('Quick actions');
  const clip = '[data-action="toggle-clipboard"]';
  const clipBefore = await page.getAttribute(clip, 'aria-checked');
  await page.click(clip);
  await page.waitForTimeout(150);
  check((await page.getAttribute(clip, 'aria-checked')) !== clipBefore, 'clipboard toggle flips');
  check((await toastText(page)).startsWith('Clipboard sharing'), 'clipboard toast');

  // ---------------------------------------------------------------- send note dialog
  await page.click('[data-action="send-note"]');
  check(await page.$eval('.dialog [type="submit"]', (b) => b.disabled), 'note Send disabled while empty');
  await page.keyboard.type('Charger is in my bag');
  check(!(await page.$eval('.dialog [type="submit"]', (b) => b.disabled)), 'note Send enabled with text');
  await page.keyboard.press('Control+Enter');
  await page.waitForTimeout(400);
  check((await count(page, '.dialog')) === 0, 'Ctrl+Enter sends and closes the note dialog');
  check((await toastText(page)) === 'Sent to Galaxy S24 FE', 'note sent toast');

  // ---------------------------------------------------------------- messages
  console.log('Messages');
  await page.click('#tab-messages');
  check((await count(page, '.unread-dot')) === 0, 'opening Messages marks the open conversation read');
  check((await count(page, '#tab-messages .badge')) === 0, 'Messages badge clears after reading');
  await page.click('[data-thread="t2"]');
  check((await text(page, '#conv-header strong')) === 'Rahul', 'opening a conversation shows it');
  check(await page.evaluate(() => document.activeElement.id === 'composer-input'), 'composer focused after opening a conversation');
  check(await page.$eval('#composer-send', (b) => b.disabled), 'Send disabled while empty');
  const bubbles = await count(page, '.bubble');
  await page.keyboard.press('Enter');
  await page.keyboard.type('   ');
  await page.keyboard.press('Enter');
  check((await count(page, '.bubble')) === bubbles, 'blank message is not sent');
  await page.fill('#composer-input', '');
  await page.keyboard.type('line one');
  await page.keyboard.press('Shift+Enter');
  await page.keyboard.type('line two');
  check((await page.$eval('#composer-input', (t) => t.value)) === 'line one\nline two', 'Shift+Enter adds a new line');
  await page.keyboard.press('Enter');
  check((await count(page, '.bubble')) === bubbles + 1, 'Enter sends the message');
  check((await page.$eval('.bubble-meta.me:last-of-type', (e) => e.textContent)).includes('Sending'), 'shows Sending… first');
  await page.waitForTimeout(900);
  const lastMeta = await page.$$eval('.bubble-meta.me', (els) => els[els.length - 1].textContent);
  check(lastMeta.includes('Delivered'), `then Delivered (got "${lastMeta}")`);
  check((await page.$eval('#composer-input', (t) => t.value)) === '', 'composer cleared after sending');
  check(await page.$eval('#conv-scroll', (s) => s.scrollTop + s.clientHeight >= s.scrollHeight - 2), 'conversation scrolled to the newest message');
  check((await page.$eval('[data-thread="t2"] .thread-last', (e) => e.textContent)).startsWith('You: line one'), 'thread list shows the sent message');

  // drafts per conversation
  await page.keyboard.type('unsent draft');
  await page.click('[data-thread="t1"]');
  check((await page.$eval('#composer-input', (t) => t.value)) === '', 'other conversation has its own empty draft');
  await page.click('[data-thread="t2"]');
  check((await page.$eval('#composer-input', (t) => t.value)) === 'unsent draft', 'draft restored when returning');
  await page.fill('#composer-input', '');

  // search, including a hostile string that must render as text
  await page.fill('#thread-search', 'hdfc');
  check((await count(page, '.thread')) === 1, 'search filters conversations');
  await page.fill('#thread-search', '<img src=x onerror=alert(1)>');
  check((await count(page, '.thread-list img')) === 0 && (await text(page, '.thread-empty')).includes('<img'), 'search text is escaped, not executed');
  await page.fill('#thread-search', '');
  check((await count(page, '.thread')) === 4, 'clearing search shows everything');

  // ---------------------------------------------------------------- files
  console.log('Files');
  const tmp = path.join(__dirname, 'notes-week-10.pdf');
  fs.writeFileSync(tmp, Buffer.alloc(40 * 1024 * 1024));
  await page.click('#tab-notifications');
  await page.setInputFiles('#file-input', tmp);
  await page.waitForTimeout(300);
  check(await page.$eval('#panel-files', (p) => !p.hidden), 'sending a file switches to Files');
  check((await text(page, '.transfer .transfer-name')) === 'notes-week-10.pdf', 'new transfer listed first');
  const p0 = await page.$eval('.transfer .progress', (b) => Number(b.getAttribute('aria-valuenow')));
  await page.waitForTimeout(700);
  const p1 = await page.$eval('.transfer .progress', (b) => Number(b.getAttribute('aria-valuenow')));
  check(p1 > p0, `progress advances (${p0} → ${p1})`);
  await page.waitForTimeout(1600);
  check((await page.$eval('.transfer', (t) => t.className)).includes('done'), 'small transfer completes');
  // choosing the same file again must work (input is reset)
  await page.setInputFiles('#file-input', tmp);
  await page.waitForTimeout(250);
  const rowsBefore = await count(page, '.transfer');
  await page.click('.transfer [data-action="cancel-transfer"]');
  await page.waitForTimeout(200);
  check((await page.$eval('.transfer', (t) => t.className)).includes('cancelled'), 'Cancel marks the transfer cancelled');
  check((await text(page, '.transfer [data-meta]')).includes('Cancelled'), 'cancelled row says so');
  check((await count(page, '.transfer')) === rowsBefore, 'it stays in the list until cleared');
  await page.waitForTimeout(700);
  check((await page.$eval('.transfer', (t) => t.className)).includes('cancelled'), 'a cancelled transfer does not restart');
  await page.click('[data-action="clear-transfers"]');
  await page.waitForTimeout(200);
  const left = await page.$$eval('.transfer', (rows) => rows.map((r) => r.className));
  check(left.every((c) => !/done|cancelled|failed/.test(c)), `Clear finished keeps only running transfers (${left.join(', ')})`);
  check(await page.evaluate(() => document.activeElement && document.activeElement.id === 'tab-files'), 'focus goes back to the Files tab after clearing');

  console.log('Earlier messages');
  await page.click('#tab-messages');
  await page.click('[data-thread="t1"]');
  await page.waitForTimeout(150);
  const bubblesBefore = await count(page, "#conv-scroll .bubble");
  await page.click('[data-action="load-older"]');
  await page.waitForTimeout(500);
  check((await count(page, '#conv-scroll .bubble')) === bubblesBefore + 2, "Load earlier messages adds the older ones");
  check((await count(page, '[data-action="load-older"]')) === 0, 'button gone when there are no more');
  await page.click('#tab-notifications');

  // ---------------------------------------------------------------- music on the phone
  console.log('Now playing');
  const np = '.now-playing';
  check(await page.$eval(np, (e) => e.offsetParent !== null).catch(() => false), 'Now Playing card shown while the phone plays');
  check((await text(page, '.np-title')) === 'Kesariya', 'song title shown');
  check((await text(page, '.np-sub')) === 'Arijit Singh · Spotify', 'artist and app shown');
  check(await page.$eval('.np-art img', (i) => i.complete && i.naturalWidth > 0), 'album art loads');
  const pos1 = await text(page, '[data-np-pos]');
  await page.waitForTimeout(2100);
  const pos2 = await text(page, '[data-np-pos]');
  check(pos1 !== pos2, `progress moves while playing (${pos1} → ${pos2})`);
  await page.click('[data-media="play_pause"]');
  check((await page.getAttribute('[data-media="play_pause"]', 'aria-label')) === 'Play', 'pause answers at once');
  await page.waitForTimeout(500);
  check((await page.getAttribute('[data-media="play_pause"]', 'aria-label')) === 'Play', 'still paused after the phone reports back');
  check(await page.evaluate(() => document.activeElement && document.activeElement.dataset.media === 'play_pause'), 'focus stays on the play button after the redraw');
  const frozen = await text(page, '[data-np-pos]');
  await page.waitForTimeout(1300);
  check((await text(page, '[data-np-pos]')) === frozen, 'progress stops while paused');
  await page.click('[data-media="next"]');
  await page.waitForTimeout(500);
  check((await text(page, '.np-title')) === 'Apna Bana Le', 'Next skips to the next song');
  check((await page.getAttribute('[data-media="play_pause"]', 'aria-label')) === 'Pause', 'new song plays');
  await page.click('[data-media="previous"]');
  await page.waitForTimeout(500);
  check((await text(page, '.np-title')) === 'Kesariya', 'Previous goes back');
  await page.$eval('[data-media-volume]', (r) => {
    r.value = '4';
    r.dispatchEvent(new Event('input', { bubbles: true }));
    r.dispatchEvent(new Event('change', { bubbles: true }));
  });
  await page.waitForTimeout(500);
  check((await page.$eval('[data-media-volume]', (r) => r.value)) === '4', 'volume slider keeps the new level after the phone reports back');
  check((await page.$eval('[data-media-volume]', (r) => r.style.getPropertyValue('--pct'))).trim() === '26.7%', 'slider fill follows the level');
  await page.focus('[data-media-volume]');
  await page.keyboard.press('ArrowRight');
  await page.waitForTimeout(500);
  check((await page.$eval('[data-media-volume]', (r) => r.value)) === '5', 'arrow keys change the volume');
  check(await page.evaluate(() => document.activeElement && document.activeElement.matches('[data-media-volume]')), 'slider keeps focus');

  // ---------------------------------------------------------------- offline behaviour
  console.log('Offline');
  await page.click('[data-preview="offline"]');
  await page.waitForTimeout(400);
  check(await page.$eval('#offline-banner', (b) => !b.hidden), 'offline banner shown');
  check((await count(page, '.now-playing')) === 0, 'no Now Playing card while the phone is offline');
  check((await text(page, '#conn-pill')).includes('Offline'), 'title pill says Offline');
  fs.writeFileSync(path.join(__dirname, 'big-video.mkv'), Buffer.alloc(1024));
  await page.setInputFiles('#file-input', path.join(__dirname, 'big-video.mkv'));
  await page.waitForTimeout(300);
  check((await page.$eval('.transfer', (t) => t.className)).includes('waiting'), 'file sent while offline waits');
  check((await page.$eval('.transfer [data-meta]', (m) => m.textContent)).includes('Waiting for Galaxy S24 FE to connect'), 'waiting transfer explains itself');
  check((await toastText(page)).includes('queued until'), 'offline send toast says queued');
  await page.waitForTimeout(800);
  check((await page.$eval('.transfer .progress', (b) => b.getAttribute('aria-valuenow'))) === '0', 'waiting transfer does not progress');
  await page.click('[data-action="troubleshoot"]');
  check((await count(page, '.dialog .tips li')) === 5, 'troubleshooting tips open');
  await page.keyboard.press('Escape');
  await page.click('#tab-messages');
  await page.fill('#composer-input', 'see you at 3');
  await page.keyboard.press('Enter');
  await page.waitForTimeout(400);
  const offMeta = await page.$$eval('.bubble-meta.me', (els) => els[els.length - 1].textContent);
  check(offMeta.includes('Queued'), `message queued while offline (got "${offMeta}")`);
  await page.click('[data-preview="connected"]');
  await page.waitForTimeout(400);
  check(await page.$eval('#offline-banner', (b) => b.hidden), 'banner disappears when back online');
  check((await count(page, '.now-playing')) === 1, 'Now Playing card returns with the phone');
  await page.click('#tab-files');
  await page.waitForTimeout(1200);
  check((await page.$$eval('.transfer', (rows) => rows.find((r) => r.querySelector('.transfer-name').textContent === 'big-video.mkv').className)).includes('done'), 'queued transfer resumes and completes after reconnect');

  // ---------------------------------------------------------------- pairing + unpair
  console.log('Pairing');
  await page.click('[data-action="pair-new"]');
  check(await page.$eval('#pairing', (p) => !p.hidden) && (await page.$eval('#main-body', (b) => b.hidden)), 'Pair another device shows the pairing screen');
  await page.click('[data-action="cancel-pairing"]');
  check(await page.$eval('#pairing', (p) => p.hidden), 'Cancel returns to the main view');

  await page.click('[data-action="open-settings"]');
  await page.waitForSelector('.sheet');
  await page.click('[data-action="unpair-ask"]');
  check(await page.evaluate(() => document.activeElement.dataset.action === 'unpair-cancel'), 'unpair asks for confirmation, focus on Keep');
  await page.keyboard.press('Enter');
  check((await count(page, '[data-action="unpair-ask"]')) === 1, 'Keep cancels unpairing');
  await page.click('[data-action="unpair-ask"]');
  await page.click('[data-action="unpair-confirm"]');
  await page.waitForTimeout(500);
  check(await page.$eval('#pairing', (p) => !p.hidden), 'after unpairing the pairing screen shows');
  check((await count(page, '[data-action="cancel-pairing"]')) === 0, 'no Cancel when nothing is paired');
  check((await text(page, '#conn-pill')) === 'No phone paired', 'pill says No phone paired');

  // QR code must decode to a EchoTwin pairing URI pointing at the documentation address
  const qrPng = PNG.sync.read(await page.locator('.qr-box').screenshot());
  const decoded = jsQR(new Uint8ClampedArray(qrPng.data), qrPng.width, qrPng.height);
  check(!!decoded && decoded.data.startsWith('phonelink:pair?v=1&') && decoded.data.includes('a=192.0.2.1:47100'), `QR scans to a safe sample pairing URI (${decoded ? decoded.data.slice(0, 40) : 'unreadable'}…)`);

  await page.click('[data-action="simulate-pair"]');
  await page.waitForTimeout(700);
  check(await page.$eval('#pairing', (p) => p.hidden), 'simulated scan pairs and returns to the main view');
  check((await toastText(page)) === 'Paired with Galaxy S24 FE', 'paired toast');

  // ---------------------------------------------------------------- rapid state switching
  console.log('Races');
  for (const m of ['offline', 'unpaired', 'connected', 'unpaired', 'offline', 'connected']) await page.click(`[data-preview="${m}"]`);
  await page.waitForTimeout(800);
  check((await text(page, '#conn-pill')).includes('Connected') && (await page.$eval('#main-body', (b) => !b.hidden)), 'rapid preview switching settles on the last choice');

  // clear all → empty state
  await page.click('#tab-notifications');
  await page.click('[data-action="clear-all"]');
  await page.waitForTimeout(200);
  check((await text(page, '#panel-notifications .empty strong')) === "You're all caught up", 'Clear all shows the empty state');
  check((await count(page, '#tab-notifications .badge')) === 0, 'badge removed when empty');

  check(page.problems.length === 0, `no console problems during interactions: ${page.problems.join(' | ')}`);
  await page.close();

  // ---------------------------------------------------------------- countdown with a fake clock
  console.log('Countdown');
  {
    const p = await browser.newPage({ viewport: { width: 1280, height: 860 } });
    await p.clock.install();
    p.problems = [];
    p.on('pageerror', (e) => p.problems.push(e.message));
    await p.goto(URL);
    await p.clock.runFor(1000);
    await p.click('[data-preview="unpaired"]');
    await p.clock.runFor(1000);
    const start = await p.$eval('[data-countdown]', (e) => e.textContent);
    check(/Expires in 1:5\d|Expires in 2:00/.test(start), `countdown starts near 2:00 (${start})`);
    await p.clock.runFor(125 * 1000);
    check((await p.$$eval('.qr-box.expired', (e) => e.length)) === 1, 'code expires after 2 minutes');
    await p.click('[data-action="new-code"]');
    await p.clock.runFor(1000);
    check((await p.$$eval('.qr-box.expired', (e) => e.length)) === 0, '"Show a new code" restarts it');
    check(p.problems.length === 0, `no errors with fake clock: ${p.problems.join(' | ')}`);
    await p.close();
  }

  // ---------------------------------------------------------------- screenshots for review (2x)
  console.log('Screenshots');
  const shot = await newPage(browser, { width: 1280, height: 860 }, 2);
  await shot.screenshot({ path: `${OUT}/01-notifications.png` });
  await shot.click('.notif[data-id="n1"] [data-action="reply-open"]');
  await shot.keyboard.type('On my way, 10 min');
  await shot.screenshot({ path: `${OUT}/02-notification-reply.png` });
  await shot.keyboard.press('Escape');
  await shot.click('#tab-messages');
  await shot.click('[data-thread="t2"]');
  await shot.waitForTimeout(150);
  await shot.screenshot({ path: `${OUT}/03-messages.png` });
  await shot.click('#tab-files');
  await shot.waitForTimeout(150);
  await shot.screenshot({ path: `${OUT}/04-files.png` });
  await shot.click('[data-action="open-settings"]');
  await shot.waitForTimeout(250);
  await shot.screenshot({ path: `${OUT}/05-settings.png` });
  await shot.keyboard.press('Escape');
  await shot.click('[data-preview="offline"]');
  await shot.waitForTimeout(400);
  await shot.click('#tab-notifications');
  await shot.screenshot({ path: `${OUT}/06-offline.png` });
  await shot.click('[data-preview="unpaired"]');
  await shot.waitForTimeout(400);
  await shot.screenshot({ path: `${OUT}/07-pairing.png` });
  await shot.close();

  await browser.close();
  console.log(`\n${passes} passed, ${failures} failed`);
  process.exit(failures ? 1 : 0);
})();
