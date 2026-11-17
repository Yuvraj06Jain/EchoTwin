// The overlay pages: the call card (call.html) and, later, the pointer (pointer.html),
// against a fake Tauri. Screenshots go to shots/.
const { chromium } = require('playwright');
const path = require('path');

const OUT = path.join(__dirname, 'shots');
let passed = 0;
let failed = 0;
const check = (cond, what) => {
  if (cond) passed++;
  else {
    failed++;
    console.log(`  FAIL: ${what}`);
  }
};

/** A fake __TAURI__ that records invokes and lets the test emit events. */
const FAKE = (status) => `
  window.__calls = [];
  window.__listeners = {};
  window.__status = ${JSON.stringify(status)};
  window.__emit = (name, payload) => (window.__listeners[name] || []).forEach((cb) => cb({ payload }));
  window.__TAURI__ = {
    core: { invoke: async (cmd, args) => { window.__calls.push([cmd, args]); if (cmd === 'status') return window.__status; return null; } },
    event: { listen: async (name, cb) => { (window.__listeners[name] = window.__listeners[name] || []).push(cb); } },
  };`;

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 452, height: 96 }, deviceScaleFactor: 2 });
  const ringing = { id: 'P', name: 'Galaxy S24 FE', phase: 'ringing', caller: 'Mom', actions: ['answer', 'decline'], active_ms: null };
  await page.addInitScript(FAKE({ call: ringing }));
  await page.goto('file://' + path.join(__dirname, '..', 'call.html'));
  await page.waitForTimeout(250);

  console.log('Call card');
  const visible = (sel) => page.$eval(sel, (e) => !e.hidden && e.getBoundingClientRect().height > 0);
  check(await visible('#card'), 'shows the ringing call from status on load');
  check((await page.textContent('#name')) === 'Mom', 'names the caller');
  check((await page.textContent('#sub')).includes('Galaxy S24 FE'), 'says which phone');
  check((await page.textContent('#av')) === 'M', 'the initial in the avatar');
  await page.screenshot({ path: `${OUT}/overlay-call-card.png` });
  await page.click('#answer');
  await page.waitForTimeout(50);
  check(JSON.stringify((await page.evaluate(() => window.__calls)).find((c) => c[0] === 'call_action')) === '["call_action",{"action":"answer"}]', 'Answer calls call_action(answer)');
  await page.click('#decline');
  await page.waitForTimeout(50);
  check((await page.evaluate(() => window.__calls.filter((c) => c[0] === 'call_action').map((c) => c[1].action))).join() === 'answer,decline', 'Decline calls call_action(decline)');

  // Only Decline offered (an older phone): no Answer button.
  await page.evaluate(() => window.__emit('phonelink://event', { kind: 'call_changed', id: 'P', name: 'Galaxy S24 FE', phase: 'ringing', caller: '', actions: ['decline'] }));
  await page.waitForTimeout(50);
  check(!(await visible('#answer')) && (await visible('#decline')), 'only the offered buttons');
  check((await page.textContent('#name')) === 'Unknown caller', 'no name: Unknown caller');

  // Answered or ended: the card hides.
  await page.evaluate(() => window.__emit('phonelink://event', { kind: 'call_changed', id: 'P', name: 'Galaxy S24 FE', phase: 'active', caller: 'Mom', actions: ['end'] }));
  await page.waitForTimeout(50);
  check(!(await visible('#card')), 'hides once the call is answered');

  await browser.close();
  console.log(`\n${passed} passed, ${failed} failed`);
  process.exit(failed ? 1 : 0);
})();
