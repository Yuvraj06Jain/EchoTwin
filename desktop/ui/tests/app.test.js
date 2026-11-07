// Tests the desktop-app code path (tauri-api.js + app.js) against a scripted fake of
// window.__TAURI__, so every backend situation can be produced on demand.
const { chromium } = require('playwright');
const path = require('path');

const URL = 'file://' + path.resolve(__dirname, '..', 'index.html');
const OUT = path.join(__dirname, 'shots');
let failures = 0;
let passes = 0;
const check = (cond, what) => {
  if (cond) passes++;
  else {
    failures++;
    console.log(`  FAIL: ${what}`);
  }
};

// Runs in the page before any app script.
function fakeTauri() {
  const calls = [];
  const listeners = {};
  const fake = {
    daemonUp: false,
    devices: [],
    notifications: true,
    deliveredNow: true,
    pairingCount: 0,
    sync: false,
    clip: true,
    lock: true,
    bt: true,
    btStatus: 'no Bluetooth adapter found (is Bluetooth on?) <b>',
    media: [],
    transfers: [],
    pick: [],
    sms: [],
    notifs: [],
    apps: [],
    muted: [],
  };
  const win = {};
  for (const m of ['minimize', 'toggleMaximize', 'close', 'startDragging', 'startResizeDragging']) {
    win[m] = async (...a) => calls.push([`window.${m}`, ...a]);
  }
  win.isMaximized = async () => false;
  win.onResized = async () => () => {};
  window.__TAURI__ = {
    window: { getCurrentWindow: () => win },
    event: {
      listen: async (name, cb) => {
        (listeners[name] = listeners[name] || []).push(cb);
        return () => {};
      },
    },
    core: {
      invoke: async (cmd, args) => {
        calls.push([cmd, args || null]);
        await new Promise((r) => setTimeout(r, 20));
        if (cmd === 'start_daemon') {
          fake.daemonUp = true;
          return null;
        }
        if (!fake.daemonUp) throw 'daemon-down';
        switch (cmd) {
          case 'status':
            return { computer: { name: 'kubuntu', id: '405F-AEB0-D952-DF66' }, notifications: fake.notifications, sync_dismissals: fake.sync, clipboard_sync: fake.clip, allow_lock: fake.lock, bluetooth_backup: fake.bt, bluetooth_status: fake.btStatus, devices: fake.devices };
          case 'notifications':
            if (fake.slowNext) {
              // Snapshot the data now, answer late (like a reload that started before news arrived).
              const delay = fake.slowNext;
              fake.slowNext = 0;
              const items = fake.notifs.map((n) => ({ ...n }));
              await new Promise((r) => setTimeout(r, delay));
              return { items, apps: [] };
            }
            return {
              items: fake.notifs.filter((n) => !fake.muted.includes(n.package)),
              apps: fake.apps.map((a) => ({ ...a, muted: fake.muted.includes(a.package) })),
            };
          case 'app_icons':
            return fake.icons || {};
          case 'dismiss_notification':
            fake.notifs = fake.notifs.filter((n) => n.id !== args.id);
            return null;
          case 'clear_notifications':
            fake.notifs = [];
            return null;
          case 'reply_notification':
          case 'notification_action':
            return fake.deliveredNow;
          case 'set_sync_dismissals':
            fake.sync = args.enabled;
            return null;
          case 'set_clipboard_sync':
            if (fake.failClip) throw 'could not save';
            fake.clip = args.enabled;
            return null;
          case 'sms_threads':
            return fake.sms;
          case 'send_sms':
            if (fake.smsFails) throw 'Galaxy S24 FE is not connected; the message was not sent';
            return 'c-1';
          case 'sms_mark_read':
          case 'sms_load_older':
            return null;
          case 'transfers':
            return fake.transfers;
          case 'send_files':
            fake.transfers = [
              ...args.paths.map((p, i) => ({ id: `t-${fake.transfers.length + i}`, device: 'x', device_name: 'Galaxy S24 FE', direction: 'outgoing', name: p.split('/').pop(), size: 1000, done: 0, state: 'active', message: '', path: null, updated_ms: Date.now() })),
              ...fake.transfers,
            ];
            return args.paths.map((_, i) => `t-${i}`);
          case 'pick_files':
            return fake.pick;
          case 'cancel_transfer':
          case 'clear_transfers':
          case 'open_transfer':
            return null;
          case 'media':
            if (fake.mediaFails) throw 'media broke';
            return fake.media;
          case 'media_control':
            return null;
          case 'set_bluetooth_backup':
            fake.bt = args.enabled;
            return null;
          case 'set_allow_lock':
            fake.lock = args.enabled;
            return null;
          case 'reconnect':
            return null;
          case 'set_app_muted':
            fake.muted = args.muted ? [...fake.muted, args.package] : fake.muted.filter((p) => p !== args.package);
            return null;
          case 'start_pairing':
            fake.pairingCount++;
            return { qr: { size: 21, path: 'M0 0h7v7h-7zM14 0h7v7h-7zM0 14h7v7h-7z' }, id: '405F-AEB0-D952-DF66', expires_in: 120 };
          case 'cancel_pairing':
            return null;
          case 'send_note':
            return fake.deliveredNow;
          case 'unpair':
            fake.devices = fake.devices.filter((d) => d.id !== args.device);
            return null;
          case 'set_notifications':
            fake.notifications = args.enabled;
            return null;
          case 'set_blur':
          case 'open_blur_settings':
            return null;
          default:
            throw `unknown command ${cmd}`;
        }
      },
    },
  };
  window.__fake = fake;
  window.__calls = calls;
  window.__emit = (name, payload) => (listeners[name] || []).forEach((cb) => cb({ payload }));
}

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1180, height: 760 }, deviceScaleFactor: 1 });
  const problems = [];
  page.on('console', (m) => ['error', 'warning'].includes(m.type()) && problems.push(`${m.type()}: ${m.text()}`));
  page.on('pageerror', (e) => problems.push(`pageerror: ${e.message}`));
  await page.addInitScript(fakeTauri);
  await page.goto(URL);
  await page.waitForTimeout(300);

  const visible = (sel) => page.$eval(sel, (e) => !e.hidden && e.offsetParent !== null).catch(() => false);
  const txt = (sel) => page.$eval(sel, (e) => e.textContent.trim());
  const calls = () => page.evaluate(() => window.__calls.map((c) => c[0]));
  const lastCall = (name) => page.evaluate((n) => [...window.__calls].reverse().find((c) => c[0] === n) || null, name);
  const toast = () => page.$eval('#toast', (e) => (e.hidden ? '' : e.textContent));

  console.log('In-app chrome');
  check(await page.evaluate(() => document.body.classList.contains('in-app')), 'body marked in-app');
  check(!(await visible('.preview-bar')), 'preview switcher hidden in the app');
  const win = await page.$eval('#window', (w) => [w.getBoundingClientRect().width, w.getBoundingClientRect().height]);
  check(win[0] === 1180 && win[1] === 760, `window fills the native window (${win})`);

  console.log('Service not running');
  check(await visible('#service'), 'service screen shown when phonelinkd is down');
  check((await txt('#conn-pill')) === 'Service not running', 'pill says Service not running');
  await page.screenshot({ path: `${OUT}/app-1-service-down.png` });
  await page.click('[data-action="start-daemon"]');
  await page.waitForTimeout(400);
  check((await calls()).includes('start_daemon'), 'Start runs start_daemon');

  console.log('Pairing (nothing paired yet)');
  check(await visible('#pairing'), 'with no devices the pairing screen shows');
  check((await page.$$eval('.qr-box svg path', (p) => p.length)) === 1, 'QR from the service is drawn');
  check((await page.$$eval('[data-action="cancel-pairing"]', (b) => b.length)) === 0, 'no Cancel when nothing is paired');
  check((await page.$$eval('[data-action="simulate-pair"]', (b) => b.length)) === 0, 'no preview-only "Simulate" button in the app');
  check((await page.evaluate(() => window.__fake.pairingCount)) === 1, 'exactly one pairing request');
  await page.evaluate(() => window.__emit('phonelink://event', { kind: 'disconnected', id: 'x', name: 'x' }));
  await page.waitForTimeout(400);
  check((await page.evaluate(() => window.__fake.pairingCount)) === 1, 'reloads do not restart pairing (the QR stays valid)');
  await page.screenshot({ path: `${OUT}/app-2-pairing.png` });

  await page.evaluate(() => window.__emit('phonelink://pairing', { kind: 'expired' }));
  await page.waitForTimeout(100);
  check((await page.$$eval('.qr-box.expired', (e) => e.length)) === 1, 'expired event shows the expired state');
  await page.click('[data-action="new-code"]');
  await page.waitForTimeout(200);
  check((await page.evaluate(() => window.__fake.pairingCount)) === 2, '"Show a new code" asks the service for a new one');

  await page.evaluate(() => {
    window.__fake.devices = [{ id: '9C21-7E04-B3A8-51DF', fingerprint: 'ab', name: 'Galaxy S24 FE', connected: true, pending: 0, last_addrs: [] }];
    window.__emit('phonelink://pairing', { kind: 'paired', name: 'Galaxy S24 FE' });
  });
  await page.waitForTimeout(400);
  check(await visible('#main-body'), 'paired event switches to the main view');
  check((await toast()) === 'Paired with Galaxy S24 FE', 'paired toast');
  check((await txt('#conn-pill')).includes('Galaxy S24 FE · Connected'), 'pill shows the connected phone');

  console.log('Only real features are offered');
  check((await page.$$eval('#device-panel [data-action="send-file"]', (b) => b.length)) === 1, 'Send files on the phone panel');
  check((await page.$$eval('[data-action="toggle-clipboard"]', (b) => b.length)) === 1, 'clipboard sharing switch on the phone panel');
  check((await page.$$eval('.battery', (b) => b.length)) === 0, 'no made-up battery level');
  check((await txt('.device-model')) === 'Paired phone', 'no made-up phone model');
  check((await txt('#panel-notifications .empty')).includes('New notifications from Galaxy S24 FE'), 'notifications empty state');
  await page.click('#tab-messages');
  check(!(await visible('#panel-messages .soon')), 'Messages tab is live');
  check((await txt('#thread-list')).includes('allow SMS access'), 'empty list says how to turn SMS on');
  await page.click('#tab-files');
  check((await txt('#panel-files')).includes('Drop files here'), 'Files tab is live');
  await page.click('#tab-notifications');
  await page.screenshot({ path: `${OUT}/app-3-connected-empty.png` });

  console.log('Notes from the phone');
  await page.evaluate(() => window.__emit('phonelink://event', { kind: 'text', id: '9C21-7E04-B3A8-51DF', name: 'Galaxy S24 FE', text: 'Leaving the lab now <b>bold?</b>' }));
  await page.waitForTimeout(200);
  check((await page.$$eval('.notif', (n) => n.length)) === 1, 'a note from the phone appears');
  check((await txt('.notif-text')) === 'Leaving the lab now <b>bold?</b>', 'note text shown as plain text');
  check((await txt('#tab-notifications .badge')) === '1', 'badge counts it');
  check((await page.$$eval('.notif [data-action="reply-open"]', (b) => b.length)) === 0, 'no Reply on notes (not possible yet)');
  await page.screenshot({ path: `${OUT}/app-4-note.png` });
  await page.click('.notif [data-action="dismiss"]');
  await page.waitForTimeout(300);
  check((await page.$$eval('.notif', (n) => n.length)) === 0, 'dismissing a note works');

  console.log('Mirrored notifications');
  const ICON = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==';
  await page.evaluate((icon) => {
    const now = Date.now();
    window.__fake.apps = [{ package: 'com.whatsapp', app: 'WhatsApp', muted: false }];
    window.__fake.icons = { 'com.whatsapp': icon };
    window.__fake.notifs = [
      { id: 'P/k1', device: 'P', device_name: 'Galaxy S24 FE', package: 'com.whatsapp', app: 'WhatsApp', title: 'Rahul', text: 'Coming to the lab?', posted_ms: now, can_reply: true, actions: ['Mark as read'], removed_on_phone: false },
      { id: 'P/k2', device: 'P', device_name: 'Galaxy S24 FE', package: 'com.whatsapp', app: 'WhatsApp', title: 'Mom', text: 'Call me', posted_ms: now - 60000, can_reply: true, actions: [], removed_on_phone: true },
    ];
    window.__emit('phonelink://event', { kind: 'app_icon', package: 'com.whatsapp' });
    window.__emit('phonelink://event', { kind: 'notification_posted', item: {} });
  }, ICON);
  await page.waitForTimeout(400);
  check((await page.$$eval('.notif', (n) => n.length)) === 2, 'mirrored notifications listed');
  check((await page.$$eval('img.app-icon', (i) => i.length)) === 1, "the app's real icon replaces the letter badge");
  check((await txt('.notif[data-id="P/k2"] .notif-actions')).includes('Cleared on phone'), 'cleared-on-phone marker shown');
  check((await page.$$eval('.notif[data-id="P/k2"] [data-action="reply-open"]', (b) => b.length)) === 0, 'no Reply once cleared on the phone');
  await page.screenshot({ path: `${OUT}/app-4b-mirrored.png` });

  await page.click('.notif[data-id="P/k1"] [data-action="reply-open"]');
  await page.keyboard.type('On my way');
  await page.keyboard.press('Enter');
  await page.waitForTimeout(300);
  const rep = await lastCall('reply_notification');
  check(rep && rep[1].id === 'P/k1' && rep[1].text === 'On my way', `reply goes to the service (${JSON.stringify(rep)})`);
  check((await toast()) === 'Reply sent to Rahul', 'reply toast');
  await page.click('.notif[data-id="P/k1"] [data-action="notif-button"][data-index="0"]');
  await page.waitForTimeout(250);
  const act = await lastCall('notification_action');
  check(act && act[1].id === 'P/k1' && act[1].index === 0, 'action button goes to the service');
  check((await toast()).includes('Mark as read'), 'action toast names the button');
  await page.evaluate(() => window.__emit('phonelink://event', { kind: 'action_result', id: 'P/k1', ok: false, message: 'That notification is no longer on your phone' }));
  await page.waitForTimeout(100);
  check((await toast()).startsWith("Your phone couldn't do that"), 'phone-side failure is reported');

  await page.focus('.notif[data-id="P/k1"] [data-action="notif-button"]');
  await page.evaluate(() => window.__emit('phonelink://event', { kind: 'notification_posted', item: {} }));
  await page.waitForTimeout(300);
  check(await page.evaluate(() => document.activeElement.dataset.action === 'notif-button'), 'focus survives a live update');

  await page.click('.notif[data-id="P/k2"] [data-action="dismiss"]');
  await page.waitForTimeout(400);
  const dis = await lastCall('dismiss_notification');
  check(dis && dis[1].id === 'P/k2', 'dismiss goes to the service');
  check((await page.$$eval('.notif', (n) => n.length)) === 1, 'dismissed one is gone');

  console.log('Race: slow reload vs live update');
  await page.evaluate(() => {
    window.__fake.slowNext = 700; // the reload's list request is slow and sees old data
    window.__emit('phonelink://event', { kind: 'connected', id: 'x', name: 'x' }); // triggers a full reload
  });
  await page.waitForTimeout(250);
  await page.evaluate(() => {
    window.__fake.notifs = [
      { id: 'P/new', device: 'P', device_name: 'Galaxy S24 FE', package: 'com.whatsapp', app: 'WhatsApp', title: 'Late news', text: 'arrived during the reload', posted_ms: Date.now(), can_reply: false, actions: [], removed_on_phone: false },
      ...window.__fake.notifs,
    ];
    window.__emit('phonelink://event', { kind: 'notification_posted', item: {} });
  });
  await page.waitForTimeout(1200);
  check((await page.$$eval('.notif[data-id="P/new"]', (n) => n.length)) === 1, 'a slow reload does not overwrite a newer live update');

  console.log('Send a note');
  await page.click('[data-action="send-note"]');
  await page.keyboard.type('Charger is in my bag');
  await page.keyboard.press('Control+Enter');
  await page.waitForTimeout(300);
  const sent = await lastCall('send_note');
  check(sent && sent[1].text === 'Charger is in my bag' && sent[1].device === '9C21-7E04-B3A8-51DF', `send_note called with text and device (${JSON.stringify(sent)})`);
  check((await toast()) === 'Sent to Galaxy S24 FE', 'sent toast');
  await page.evaluate(() => (window.__fake.deliveredNow = false));
  await page.click('[data-action="send-note"]');
  await page.keyboard.type('later');
  await page.keyboard.press('Control+Enter');
  await page.waitForTimeout(300);
  check((await toast()).startsWith('Queued'), 'queued toast when the phone is away');

  await page.click('[data-action="clear-all"]');
  await page.waitForTimeout(300);
  check((await calls()).includes('clear_notifications'), 'Clear all goes to the service');
  check((await page.$$eval('.notif', (n) => n.length)) === 0, 'list empty after Clear all');
  await page.evaluate(() => {
    window.__fake.notifs = [{ id: 'P/k3', device: 'P', device_name: 'Galaxy S24 FE', package: 'com.whatsapp', app: 'WhatsApp', title: 'Ananya', text: 'Notes uploaded', posted_ms: Date.now(), can_reply: false, actions: [], removed_on_phone: false }];
    window.__emit('phonelink://event', { kind: 'notification_posted', item: {} });
  });
  await page.waitForTimeout(300);

  console.log('Settings');
  await page.click('[data-action="open-settings"]');
  check((await page.$$eval('.sheet [data-action="toggle-app"]', (b) => b.length)) === 1, 'per-app switch for the app seen so far');
  check((await page.getAttribute('.sheet [data-key="syncDismissals"]', 'aria-checked')) === 'false', '"Dismiss on both devices" starts off');
  await page.click('.sheet [data-key="syncDismissals"]');
  await page.waitForTimeout(150);
  const setS = await lastCall('set_sync_dismissals');
  check(setS && setS[1].enabled === true, 'sync switch calls set_sync_dismissals(true)');
  await page.click('.sheet [data-action="toggle-app"][data-name="WhatsApp"]');
  await page.waitForTimeout(250);
  const mute = await lastCall('set_app_muted');
  check(mute && mute[1].package === 'com.whatsapp' && mute[1].muted === true, `muting calls set_app_muted (${JSON.stringify(mute)})`);
  await page.click('.sheet [data-action="toggle-app"][data-name="WhatsApp"]');
  await page.waitForTimeout(250);
  check((await page.$$eval('.sheet [data-key="discovery"]', (b) => b.length)) === 0, 'no discovery toggle yet');

  console.log('Clipboard and lock settings');
  check((await page.getAttribute('.sheet [data-key="clipboardSync"]', 'aria-checked')) === 'true', 'clipboard sharing shows the service value (on)');
  check((await page.getAttribute('.sheet [data-key="allowLock"]', 'aria-checked')) === 'true', 'lock permission shows the service value (on)');
  check((await page.getAttribute('.sheet [data-key="bluetoothBackup"]', 'aria-checked')) === 'true', 'Bluetooth backup switch shows the service value');
  check((await txt('.sheet')).includes('Not available right now: no Bluetooth adapter found'), 'says why the backup is not ready');
  check(!(await page.$eval('.sheet', (e) => e.innerHTML)).includes('<b>'), 'service text is escaped');
  await page.click('.sheet [data-key="bluetoothBackup"]');
  await page.waitForTimeout(150);
  const setB = await lastCall('set_bluetooth_backup');
  check(setB && setB[1].enabled === false, 'Bluetooth switch calls set_bluetooth_backup(false)');
  await page.click('.sheet [data-key="allowLock"]');
  await page.waitForTimeout(150);
  const setL = await lastCall('set_allow_lock');
  check(setL && setL[1].enabled === false, 'lock switch calls set_allow_lock(false)');
  await page.click('.sheet [data-key="clipboardSync"]');
  await page.waitForTimeout(150);
  const setC = await lastCall('set_clipboard_sync');
  check(setC && setC[1].enabled === false, 'clipboard switch calls set_clipboard_sync(false)');
  await page.keyboard.press('Escape');
  await page.waitForTimeout(150);
  check((await page.getAttribute('[data-action="toggle-clipboard"]', 'aria-checked')) === 'false', 'phone panel clipboard switch follows the setting');
  // A failed save puts the switch back.
  await page.evaluate(() => { window.__fake.failClip = true; });
  await page.click('[data-action="toggle-clipboard"]');
  await page.waitForTimeout(250);
  check((await page.getAttribute('[data-action="toggle-clipboard"]', 'aria-checked')) === 'false', 'failed save reverts the switch');
  check((await toast()).length > 0, 'failure is reported');
  await page.evaluate(() => { window.__fake.failClip = false; });
  await page.click('[data-action="toggle-clipboard"]');
  await page.waitForTimeout(250);
  check((await page.evaluate(() => window.__fake.clip)) === true, 'phone panel switch turns sharing back on');
  await page.evaluate(() => window.__emit('phonelink://event', { kind: 'clipboard_received', id: 'x', name: 'Galaxy S24 FE' }));
  await page.waitForTimeout(150);
  check((await toast()).startsWith('Copied from Galaxy S24 FE'), 'toast when the phone sends its clipboard');
  await page.click('[data-action="open-settings"]');
  const notifSwitch = '.sheet [data-key="desktopNotifications"]';
  check((await page.getAttribute(notifSwitch, 'aria-checked')) === 'true', 'notifications switch reflects the service');
  await page.click(notifSwitch);
  await page.waitForTimeout(150);
  const setN = await lastCall('set_notifications');
  check(setN && setN[1].enabled === false, 'switch calls set_notifications(false)');
  check(!!(await lastCall('set_blur')) && (await lastCall('set_blur'))[1].on === true, 'blur requested at startup (default on)');
  const alpha = () => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--alpha').trim());
  check((await alpha()) === '0.85', 'default window opacity 85%: ' + (await alpha()));
  await page.$eval('.sheet [data-opacity]', (el) => { el.value = '60'; el.dispatchEvent(new Event('input', { bubbles: true })); });
  check((await alpha()) === '0.6', 'opacity slider applies live');
  check((await txt('.sheet [data-opacity-value]')) === '60%', 'shows the value');
  check(await page.evaluate(() => localStorage.getItem('phonelink.opacity') === '60'), 'opacity remembered');
  const winBg = await page.evaluate(() => getComputedStyle(document.querySelector('.window')).backgroundColor);
  check(winBg.includes('0.6'), 'window background is translucent: ' + winBg);
  const textColor = await page.evaluate(() => getComputedStyle(document.querySelector('.device-name')).color);
  check(!textColor.startsWith('rgba'), 'text stays solid: ' + textColor);
  await page.click('.sheet [data-action="toggle-blur"]');
  await page.waitForTimeout(100);
  check((await lastCall('set_blur'))[1].on === false, 'blur switch turns it off');
  check(await page.evaluate(() => localStorage.getItem('phonelink.blur') === 'off'), 'blur choice remembered');
  await page.click('.sheet [data-action="open-blur-settings"]');
  await page.waitForTimeout(100);
  check(!!(await lastCall('open_blur_settings')), 'opens KDE blur settings');
  await page.$eval('.sheet-body', (el) => { el.scrollTop = el.scrollHeight; });
  await page.screenshot({ path: `${OUT}/app-5b-appearance.png` });
  await page.click('.sheet [data-action="toggle-blur"]');
  await page.$eval('.sheet [data-opacity]', (el) => { el.value = '85'; el.dispatchEvent(new Event('input', { bubbles: true })); });
  await page.screenshot({ path: `${OUT}/app-5-settings.png` });
  await page.keyboard.press('Escape');

  console.log('Window controls');
  await page.click('[data-action="win-minimize"]');
  await page.click('[data-action="win-maximize"]');
  await page.click('[data-action="win-close"]');
  const c = await calls();
  check(c.includes('window.minimize') && c.includes('window.toggleMaximize') && c.includes('window.close'), 'title-bar buttons drive the real window');
  await page.mouse.move(700, 18);
  await page.mouse.down();
  await page.mouse.up();
  check((await calls()).includes('window.startDragging'), 'dragging the title bar moves the window');
  await page.mouse.dblclick(700, 18);
  check((await calls()).filter((x) => x === 'window.toggleMaximize').length >= 2, 'double-click on the title bar maximises');
  await page.mouse.move(1178, 380);
  await page.mouse.down();
  await page.mouse.up();
  const rs = await lastCall('window.startResizeDragging');
  check(rs && rs[1] === 'East', `right edge resizes the window (${JSON.stringify(rs)})`);

  console.log('Service stops and comes back');
  await page.evaluate(() => {
    window.__fake.daemonUp = false;
    window.__emit('phonelink://daemon', false);
  });
  await page.waitForTimeout(100);
  check(await visible('#service'), 'service screen when phonelinkd stops');
  await page.evaluate(() => {
    window.__fake.daemonUp = true;
    window.__emit('phonelink://daemon', true);
  });
  await page.waitForTimeout(500);
  check(await visible('#main-body'), 'back to the main view when it returns');

  console.log('Reconnect now');
  check((await page.$$eval('[data-action="reconnect"]', (b) => b.filter((x) => x.offsetParent).length)) === 0, 'no Reconnect button while connected');
  await page.evaluate(() => {
    window.__fake.devices = window.__fake.devices.map((d) => ({ ...d, connected: false }));
    window.__emit('phonelink://event', { kind: 'disconnected', id: 'x', name: 'x' });
  });
  await page.waitForTimeout(400);
  check(await visible('[data-action="reconnect"]'), 'Reconnect now shown while offline');
  await page.click('[data-action="reconnect"]');
  await page.waitForTimeout(200);
  check((await calls()).includes('reconnect'), 'Reconnect now asks the service');
  check((await toast()).startsWith('Looking for Galaxy S24 FE'), 'reconnect toast');
  await page.evaluate(() => {
    window.__fake.devices = window.__fake.devices.map((d) => ({ ...d, connected: true }));
    window.__emit('phonelink://event', { kind: 'connected', id: 'x', name: 'x' });
  });
  await page.waitForTimeout(400);
  check(!(await visible('#offline-banner')), 'banner gone once reconnected');

  console.log('Popup clicked while the window is open');
  const selectedTab = () => page.$eval('[data-tab][aria-selected="true"]', (t) => t.dataset.tab);
  await page.click('[data-tab="files"]');
  await page.click('[data-action="open-settings"]');
  check(await visible('.sheet'), 'settings open before the click');
  await page.evaluate(() => window.__emit('phonelink://open', { tab: 'notifications' }));
  await page.waitForTimeout(300);
  check(!(await visible('.sheet')), 'settings close for the popup');
  check((await selectedTab()) === 'notifications', 'notifications tab shown after a popup click');
  await page.click('[data-tab="messages"]');
  await page.evaluate(() => window.__emit('phonelink://open', { tab: 'bogus' }));
  await page.evaluate(() => window.__emit('phonelink://open', null));
  await page.waitForTimeout(200);
  check((await selectedTab()) === 'messages', 'unknown or empty requests change nothing');
  await page.click('[data-tab="notifications"]');

  console.log('SMS');
  const thread = (extra) => ({ id: 'P/3', device: 'P', address: '+919800004410', name: 'Mom', unread: 1, has_older: true,
    messages: [
      { id: '30', from_me: false, body: 'Did you reach?', date_ms: Date.now() - 600000, state: 'received', error: '' },
      { id: '31', from_me: false, body: 'Call me <b>now</b>', date_ms: Date.now() - 300000, state: 'received', error: '' },
      ...(extra || []),
    ] });
  await page.evaluate((t) => {
    window.__fake.sms = [t];
    window.__emit('phonelink://event', { kind: 'sms_changed', thread: null });
  }, thread());
  await page.waitForTimeout(300);
  check((await page.$eval('#tab-messages .badge', (b) => b.textContent).catch(() => '')) === '1', 'unread badge on the Messages tab');
  await page.click('#tab-messages');
  await page.waitForTimeout(200);
  check((await txt('.thread .thread-name')) === 'Mom', 'conversation listed');
  check((await page.$$eval('.bubble', (b) => b.map((x) => x.innerHTML))).includes('Call me &lt;b&gt;now&lt;/b&gt;'), 'message text is escaped');
  let mr = await lastCall('sms_mark_read');
  check(mr && mr[1].thread === 'P/3', 'opening it marks it read on this computer');
  await page.fill('#composer-input', 'Reached. Calling at 8');
  await page.keyboard.press('Enter');
  await page.waitForTimeout(200);
  const ss = await lastCall('send_sms');
  check(ss && ss[1].thread === 'P/3' && ss[1].text === 'Reached. Calling at 8', 'Enter sends through the phone');
  check((await page.$$eval('.bubble-meta.me', (m) => m.map((x) => x.textContent))).some((t) => t.includes('Sending')), 'shown as sending');
  // The phone's copy arrives (as the service reports it).
  await page.evaluate((t) => {
    window.__fake.sms = [t];
    window.__emit('phonelink://event', { kind: 'sms_send_result', client_id: 'c-1', state: 'sent', error: '' });
  }, thread([{ id: '32', from_me: true, body: 'Reached. Calling at 8', date_ms: Date.now(), state: 'sent', error: '' }]));
  await page.waitForTimeout(300);
  check((await page.$$eval('.bubble.me', (b) => b.filter((x) => x.textContent === 'Reached. Calling at 8').length)) === 1, 'one bubble, not two');
  check((await page.$$eval('.bubble-meta.me', (m) => m.map((x) => x.textContent))).some((t) => t.includes('Sent')), 'marked sent');
  // Earlier messages.
  await page.click('[data-action="load-older"]');
  await page.waitForTimeout(100);
  check((await lastCall('sms_load_older'))[1].thread === 'P/3', 'Load earlier asks the phone');
  // Failures are explained and the text isn't silently lost.
  await page.evaluate(() => { window.__fake.smsFails = true; });
  await page.fill('#composer-input', 'hello?');
  await page.keyboard.press('Enter');
  await page.waitForTimeout(300);
  check((await toast()).includes('not connected'), 'offline send explained');
  check((await page.$$eval('.bubble-meta.me', (m) => m.map((x) => x.textContent))).some((t) => t.includes('Not sent')), 'failed bubble marked');
  await page.evaluate(() => { window.__fake.smsFails = false; });
  await page.evaluate(() => window.__emit('phonelink://event', { kind: 'sms_send_result', client_id: 'x', state: 'failed', error: 'No service' }));
  await page.waitForTimeout(200);
  check((await toast()).includes('No service'), 'phone-side failure reason shown');

  console.log('Files');
  await page.click('#tab-files');
  // Picker cancelled: nothing is sent.
  await page.evaluate(() => { window.__fake.pick = []; });
  await page.click('#panel-files [data-action="send-file"]');
  await page.waitForTimeout(200);
  check(!(await calls()).includes('send_files'), 'cancelling the picker sends nothing');
  await page.evaluate(() => { window.__fake.pick = ['/home/y/Documents/OS lab report.pdf']; });
  await page.click('#panel-files [data-action="send-file"]');
  await page.waitForTimeout(300);
  let sf = await lastCall('send_files');
  check(sf && sf[1].paths[0] === '/home/y/Documents/OS lab report.pdf', `chosen file is sent by path (${JSON.stringify(sf)})`);
  check((await txt('.transfer .transfer-name')) === 'OS lab report.pdf', 'it is listed');
  check((await toast()).startsWith('Sending “OS lab report.pdf”'), 'sending toast');
  // Progress arrives as events and updates the row in place.
  await page.focus('.transfer [data-action="cancel-transfer"]');
  await page.evaluate(() => window.__emit('phonelink://event', { kind: 'transfer_changed', item: { id: 't-0', device: 'x', device_name: 'Galaxy S24 FE', direction: 'outgoing', name: 'OS lab report.pdf', size: 1000, done: 640, state: 'active', message: '', path: null, updated_ms: 1 } }));
  await page.waitForTimeout(100);
  check((await page.getAttribute('.transfer .progress', 'aria-valuenow')) === '64', 'progress bar follows the events');
  check(await page.evaluate(() => document.activeElement && document.activeElement.dataset.action === 'cancel-transfer'), 'focus kept during progress updates');
  await page.evaluate(() => window.__emit('phonelink://event', { kind: 'transfer_changed', item: { id: 't-0', device: 'x', device_name: 'Galaxy S24 FE', direction: 'outgoing', name: 'OS lab report.pdf', size: 1000, done: 640, state: 'waiting', message: 'Waiting for Wi-Fi (connected over Bluetooth)', path: null, updated_ms: 2 } }));
  await page.waitForTimeout(100);
  check((await txt('.transfer [data-meta]')).includes('64% · Waiting for Wi-Fi'), 'waiting shows why');
  // A file from the phone: Open and Show in folder.
  await page.evaluate(() => window.__emit('phonelink://event', { kind: 'transfer_changed', item: { id: 'in-1', device: 'x', device_name: 'Galaxy S24 FE', direction: 'incoming', name: 'IMG_1.jpg', size: 5000, done: 5000, state: 'done', message: '', path: '/home/y/Downloads/PhoneLink/IMG_1.jpg', updated_ms: 3 } }));
  await page.waitForTimeout(150);
  check((await toast()) === 'Received “IMG_1.jpg”' || (await txt('.transfer[data-id="in-1"] [data-meta]')).includes('Saved to Downloads/PhoneLink'), 'received file shown');
  await page.click('.transfer[data-id="in-1"] [data-action="show-in-folder"]');
  await page.waitForTimeout(100);
  let ot = await lastCall('open_transfer');
  check(ot && ot[1].id === 'in-1' && ot[1].reveal === true, 'Show in folder asks the service to reveal it');
  await page.click('.transfer[data-id="in-1"] [data-action="open-transfer"]');
  await page.waitForTimeout(100);
  ot = await lastCall('open_transfer');
  check(ot && ot[1].reveal === false, 'Open asks the service to open it');
  await page.evaluate(() => window.__emit('phonelink://event', { kind: 'transfer_changed', item: { id: 'x-9', device: 'x', device_name: 'Galaxy S24 FE', direction: 'outgoing', name: 'huge.iso', size: 9e9, done: 0, state: 'failed', message: 'Not enough free space on Galaxy S24 FE', path: null, updated_ms: 4 } }));
  await page.waitForTimeout(100);
  check((await page.$eval('.transfer[data-id="x-9"]', (r) => r.className)).includes('failed'), 'failed transfer marked');
  check((await txt('.transfer[data-id="x-9"] [data-meta]')).includes('Not enough free space'), 'failure explains itself');
  // Files dragged onto the window arrive as paths from the desktop app.
  await page.click('#tab-notifications');
  await page.evaluate(() => window.__emit('phonelink://drop', { kind: 'enter' }));
  await page.waitForTimeout(100);
  check(await page.evaluate(() => document.getElementById('window').classList.contains('dragging')), 'drop zone highlighted while dragging');
  check((await page.$eval('[data-tab][aria-selected="true"]', (t) => t.dataset.tab)) === 'files', 'dragging over the window opens Files');
  await page.evaluate(() => window.__emit('phonelink://drop', { kind: 'drop', paths: ['/home/y/a.png', '/home/y/b.png'] }));
  await page.waitForTimeout(250);
  check(!(await page.evaluate(() => document.getElementById('window').classList.contains('dragging'))), 'highlight goes after the drop');
  sf = await lastCall('send_files');
  check(sf && sf[1].paths.length === 2 && sf[1].paths[1] === '/home/y/b.png', 'dropped files are sent by path');
  check((await toast()).startsWith('Sending 2 files'), 'toast for several files');
  await page.evaluate(() => window.__emit('phonelink://drop', { kind: 'enter' }));
  await page.evaluate(() => window.__emit('phonelink://drop', { kind: 'leave' }));
  await page.waitForTimeout(100);
  check(!(await page.evaluate(() => document.getElementById('window').classList.contains('dragging'))), 'leaving the window removes the highlight');
  await page.click('#tab-notifications');

  console.log('Bluetooth link');
  await page.evaluate(() => {
    window.__fake.devices = window.__fake.devices.map((d) => ({ ...d, via: 'bluetooth' }));
    window.__emit('phonelink://event', { kind: 'connected', id: 'x', name: 'x', via: 'bluetooth' });
  });
  await page.waitForTimeout(400);
  check((await txt('#conn-pill')).endsWith('Connected · Bluetooth'), 'title bar shows the Bluetooth link');
  check((await txt('#device-panel .stats')).includes('Connected over Bluetooth'), 'phone panel says so');
  await page.evaluate(() => {
    window.__fake.devices = window.__fake.devices.map((d) => ({ ...d, via: 'wifi' }));
    window.__emit('phonelink://event', { kind: 'connected', id: 'x', name: 'x', via: 'wifi' });
  });
  await page.waitForTimeout(400);
  check((await txt('#conn-pill')).endsWith('· Connected'), 'back to Wi-Fi');

  console.log('Phone battery and network');
  check(!(await txt('#device-panel .stats')).includes('%'), 'no battery shown before the phone reports it');
  await page.evaluate(() => {
    window.__fake.devices = window.__fake.devices.map((d) => ({ ...d, status: { battery: 82, charging: true, network: '5G', signal: 3 } }));
    window.__emit('phonelink://event', { kind: 'phone_status_changed', id: '9C21-7E04-B3A8-51DF' });
  });
  await page.waitForTimeout(400);
  const stats = await txt('#device-panel .stats');
  check(stats.includes('82% · charging') && stats.includes('5G'), 'battery and 5G shown: ' + stats.replace(/\s+/g, ' '));
  check((await page.$$eval('#device-panel .bars > i.on', (e) => e.length)) === 3, 'three of four signal bars lit');
  await page.evaluate(() => {
    window.__fake.devices = window.__fake.devices.map((d) => ({ ...d, status: { battery: 12, charging: false, network: 'Wi-Fi', signal: 0 } }));
    window.__emit('phonelink://event', { kind: 'phone_status_changed', id: '9C21-7E04-B3A8-51DF' });
  });
  await page.waitForTimeout(400);
  check((await page.$$eval('#device-panel .battery.low', (e) => e.length)) === 1, 'low battery is marked');
  check((await page.$$eval('#device-panel .bars', (e) => e.length)) === 0, 'no bars on Wi-Fi');
  await page.screenshot({ path: 'shots/app-phone-status.png' });

  console.log('Music');
  check((await page.$$eval('.now-playing', (e) => e.length)) === 0, 'no card while nothing plays');
  await page.evaluate(() => {
    window.__fake.media = [{ device: '9C21-7E04-B3A8-51DF', device_name: 'Galaxy S24 FE', app: 'YouTube Music', package: 'com.google.android.apps.youtube.music', title: 'Tum Hi Ho', artist: '', album: '', duration_ms: 0, position_ms: 0, playing: false, can_previous: false, can_next: true, volume: 3, max_volume: 15, art: null }];
    window.__emit('phonelink://event', { kind: 'media_changed', id: '9C21-7E04-B3A8-51DF' });
  });
  await page.waitForTimeout(300);
  check(await visible('.now-playing'), 'card appears when the phone starts a player');
  check((await txt('.np-sub')) === 'YouTube Music', 'no stray separator without an artist');
  check((await page.$$eval('.np-progress', (e) => e.length)) === 0, 'no progress bar for unknown length (live stream)');
  check(await page.$eval('[data-media="previous"]', (b) => b.disabled), 'Previous disabled when the player has none');
  check((await page.$$eval('.np-art svg', (e) => e.length)) === 1, 'placeholder icon without art');
  await page.click('[data-media="play_pause"]');
  await page.waitForTimeout(150);
  let mc = await lastCall('media_control');
  check(mc && mc[1].action === 'play_pause', `play calls media_control("play_pause") (${JSON.stringify(mc)})`);
  await page.click('[data-media="next"]');
  await page.waitForTimeout(150);
  mc = await lastCall('media_control');
  check(mc && mc[1].action === 'next', 'next calls media_control("next")');
  await page.$eval('[data-media-volume]', (r) => {
    r.value = '11';
    r.dispatchEvent(new Event('input', { bubbles: true }));
    r.dispatchEvent(new Event('change', { bubbles: true }));
  });
  await page.waitForTimeout(150);
  mc = await lastCall('media_control');
  check(mc && mc[1].action && mc[1].action.set_volume === 11, `volume calls media_control({set_volume: 11}) (${JSON.stringify(mc)})`);
  // A late answer from an earlier refresh must not undo a newer state.
  await page.evaluate(() => {
    window.__fake.media = [];
    window.__emit('phonelink://event', { kind: 'media_changed', id: 'x' });
  });
  await page.waitForTimeout(300);
  check((await page.$$eval('.now-playing', (e) => e.length)) === 0, 'card goes when the player closes');
  await page.evaluate(() => {
    window.__fake.mediaFails = true;
    window.__emit('phonelink://event', { kind: 'media_changed', id: 'x' });
  });
  await page.waitForTimeout(300);
  check(await visible('#device-panel .device-name'), 'a failing music request leaves the rest working');
  await page.evaluate(() => { window.__fake.mediaFails = false; });

  console.log('Unpair');
  await page.click('[data-action="open-settings"]');
  await page.click('[data-action="unpair-ask"]');
  await page.click('[data-action="unpair-confirm"]');
  await page.waitForTimeout(500);
  const un = await lastCall('unpair');
  check(un && un[1].device === '9C21-7E04-B3A8-51DF', 'unpair called with the device id');
  check(await visible('#pairing'), 'pairing screen after the last device is unpaired');

  console.log('Layout in the app');
  const overflow = await page.evaluate(() => document.getElementById('window').scrollWidth > document.getElementById('window').clientWidth);
  check(!overflow, 'no horizontal overflow');
  // The one deliberate failure (a save that fails) is expected to be logged.
  const unexpected = problems.filter((p) => !p.includes('could not save'));
  check(unexpected.length === 0, `no console problems: ${unexpected.join(' | ')}`);

  console.log('Launched by a popup click');
  const page2 = await browser.newPage();
  await page2.addInitScript(fakeTauri);
  await page2.addInitScript(() => {
    window.__PHONELINK_OPEN_TAB__ = 'files';
    window.__fake.daemonUp = true;
    window.__fake.devices = [{ id: '9C21-7E04-B3A8-51DF', fingerprint: 'ab', name: 'Galaxy S24 FE', connected: true, pending: 0, last_addrs: [] }];
  });
  await page2.goto(URL);
  await page2.waitForTimeout(800);
  check((await page2.$eval('[data-tab][aria-selected="true"]', (t) => t.dataset.tab)) === 'files', 'start-up tab from the launch request');
  const page3 = await browser.newPage();
  await page3.addInitScript(fakeTauri);
  await page3.addInitScript(() => {
    window.__PHONELINK_OPEN_TAB__ = 'nonsense';
    window.__fake.daemonUp = true;
    window.__fake.devices = [{ id: '9C21-7E04-B3A8-51DF', fingerprint: 'ab', name: 'Galaxy S24 FE', connected: true, pending: 0, last_addrs: [] }];
  });
  await page3.goto(URL);
  await page3.waitForTimeout(800);
  check((await page3.$eval('[data-tab][aria-selected="true"]', (t) => t.dataset.tab)) === 'notifications', 'bad start-up tab ignored');

  await browser.close();
  console.log(`\n${passes} passed, ${failures} failed`);
  process.exit(failures ? 1 : 0);
})();
