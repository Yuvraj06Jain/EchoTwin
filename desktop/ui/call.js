// The call card at the top of the screen (overlay window "call"): who calls, Decline and
// Answer. Shown while the phone rings; the app hides the window otherwise.
//
// The window is made after the call was announced, so on load the page says
// "call-card-ready" and the app sends the call again. Once the card is really on screen the
// page says "call-card-visible": only then does the service skip its KDE notification.
(() => {
  const T = window.__TAURI__;
  const $ = (id) => document.getElementById(id);
  const COLORS = ['#b65ad8', '#4f7cff', '#d9a441', '#3dbb82', '#e5707a', '#4fb0c6'];
  const colorFor = (s) => COLORS[[...s].reduce((h, c) => (h * 31 + c.codePointAt(0)) >>> 0, 7) % COLORS.length];

  let announced = null; // the call id last reported visible

  function show(call) {
    if (!call || call.phase !== 'ringing') {
      $('card').hidden = true;
      announced = null;
      return;
    }
    const name = call.caller || 'Unknown caller';
    $('name').textContent = name;
    $('sub').textContent = `Incoming call · ${call.name || 'your phone'}`;
    $('av').textContent = call.caller ? [...call.caller.trim()][0].toUpperCase() : '?';
    $('av').style.background = colorFor(name);
    const offers = call.actions || [];
    $('answer').hidden = !offers.includes('answer');
    $('decline').hidden = !offers.includes('decline');
    const card = $('card');
    if (card.hidden) {
      card.hidden = false;
      card.classList.remove('in');
      void card.offsetWidth; // restart the slide-down
      card.classList.add('in');
    }
    if (announced !== call.id && call.id) {
      announced = call.id;
      T.event.emit('phonelink://call-card-visible', { id: call.id });
    }
  }

  async function press(action, button) {
    button.disabled = true;
    try {
      await T.core.invoke('call_action', { action });
    } catch (e) {
      $('sub').textContent = String(e);
    } finally {
      button.disabled = false;
    }
  }
  $('answer').addEventListener('click', (e) => press('answer', e.currentTarget));
  $('decline').addEventListener('click', (e) => press('decline', e.currentTarget));

  async function refresh() {
    try {
      const s = await T.core.invoke('status');
      show(s.call);
    } catch {
      /* the service is restarting: the next event redraws */
    }
  }
  T.event.listen('phonelink://event', ({ payload }) => {
    if (payload.kind === 'call_changed') show(payload);
  });
  // Ask the app for the call (it may have been announced before this page existed); the
  // service's status is a second way.
  T.event.emit('phonelink://call-card-ready');
  refresh();
})();
