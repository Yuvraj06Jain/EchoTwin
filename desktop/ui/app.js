// PhoneLink desktop UI.
// All data comes from window.PhoneLinkAPI (mock-api.js in the preview, the daemon later).
'use strict';

(function () {
  const api = window.PhoneLinkAPI;
  const win = window.PhoneLinkWindow || null; // real window controls inside the desktop app
  const caps = {
    preview: true,
    notificationMirroring: true,
    sms: true,
    files: true,
    clipboard: true,
    deviceDetails: true,
    appFilters: true,
    discoveryToggle: true,
    syncDismissals: true,
    notificationButtons: true,
    ...(api.capabilities || {}),
  };

  // ------------------------------------------------------------------ helpers

  const $ = (sel, root = document) => root.querySelector(sel);
  const ESC = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
  const esc = (v) => String(v ?? '').replace(/[&<>"']/g, (c) => ESC[c]);
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  const pad = (n) => String(n).padStart(2, '0');

  const ICONS = {
    settings: '<circle cx="12" cy="12" r="2.6"/><path d="M12 3.5v2.2M12 18.3v2.2M3.5 12h2.2M18.3 12h2.2M6 6l1.6 1.6M16.4 16.4L18 18M6 18l1.6-1.6M16.4 7.6L18 6"/><circle cx="12" cy="12" r="6.2"/>',
    minimize: '<path d="M7 12h10"/>',
    maximize: '<rect x="7" y="7" width="10" height="10" rx="1.5"/>',
    close: '<path d="M7.5 7.5l9 9M16.5 7.5l-9 9"/>',
    bell: '<path d="M6.5 16v-4.5a5.5 5.5 0 0 1 11 0V16l1.5 2h-14z"/><path d="M10 20.5a2.2 2.2 0 0 0 4 0"/>',
    chat: '<path d="M5 5h14a1.5 1.5 0 0 1 1.5 1.5v9A1.5 1.5 0 0 1 19 17h-8l-4.5 3.5V17H5a1.5 1.5 0 0 1-1.5-1.5v-9A1.5 1.5 0 0 1 5 5z"/>',
    folder: '<path d="M3.5 7.5A1.5 1.5 0 0 1 5 6h4l2 2h8a1.5 1.5 0 0 1 1.5 1.5v8A1.5 1.5 0 0 1 19 19H5a1.5 1.5 0 0 1-1.5-1.5z"/>',
    wifi: '<path d="M4 9.5a12 12 0 0 1 16 0M7 12.8a7.5 7.5 0 0 1 10 0M10 16a3 3 0 0 1 4 0"/><path d="M12 19.2v.01"/>',
    send: '<path d="M4.5 12L20 4.5 15 20l-3.2-6.8z"/><path d="M11.8 13.2L20 4.5"/>',
    upload: '<path d="M12 15.5V4.5M7.5 9L12 4.5 16.5 9"/><path d="M5 14.5v4A1.5 1.5 0 0 0 6.5 20h11a1.5 1.5 0 0 0 1.5-1.5v-4"/>',
    download: '<path d="M12 4.5v11M7.5 11L12 15.5 16.5 11"/><path d="M5 14.5v4A1.5 1.5 0 0 0 6.5 20h11a1.5 1.5 0 0 0 1.5-1.5v-4"/>',
    clipboard: '<rect x="6" y="5" width="12" height="15.5" rx="2"/><path d="M9.5 5V4a1 1 0 0 1 1-1h3a1 1 0 0 1 1 1v1M9 11h6M9 14.5h4"/>',
    check: '<path d="M5 12.5l4.5 4.5L19 7.5"/>',
    alert: '<path d="M12 4.5l8.5 15h-17z"/><path d="M12 10v4.2M12 17v.01"/>',
    phone: '<rect x="7" y="3" width="10" height="18" rx="2.2"/><path d="M11 18h2"/>',
    reply: '<path d="M9.5 7.5L5 12l4.5 4.5"/><path d="M5.5 12H14a5.5 5.5 0 0 1 5.5 5.5"/>',
    plus: '<path d="M12 5.5v13M5.5 12h13"/>',
    file: '<path d="M7 3.5h7l4 4V20a.5.5 0 0 1-.5.5h-10A.5.5 0 0 1 7 20z"/><path d="M14 3.5V8h4"/>',
  };
  const icon = (name) => `<svg class="i" viewBox="0 0 24 24" aria-hidden="true">${ICONS[name]}</svg>`;

  const APP_COLORS = { PhoneLink: '#3dbb82', WhatsApp: '#62c18e', Gmail: '#e2847b', Calendar: '#79a9e6', Messages: '#86b8d8', Swiggy: '#e6a463', Instagram: '#d491b8' };
  const PALETTE = ['#86b8d8', '#e3ab76', '#9fcb8f', '#c5a3da', '#e39393', '#80d0c4', '#d8c27e'];
  function hash(s) {
    let h = 0;
    for (const ch of String(s)) h = (h * 31 + ch.codePointAt(0)) >>> 0;
    return h;
  }
  const colorFor = (name) => APP_COLORS[name] || PALETTE[hash(name) % PALETTE.length];
  const initial = (name) => (String(name).match(/[A-Za-z0-9]/) || ['?'])[0].toUpperCase();

  const dayKey = (ts) => {
    const d = new Date(ts);
    return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
  };
  const clock = (ts) => {
    const d = new Date(ts);
    return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  };
  function isYesterday(ts) {
    const y = new Date();
    y.setDate(y.getDate() - 1);
    return dayKey(ts) === dayKey(y);
  }
  function dayLabel(ts) {
    if (dayKey(ts) === dayKey(Date.now())) return 'Today';
    if (isYesterday(ts)) return 'Yesterday';
    return new Date(ts).toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' });
  }
  function relTime(ts) {
    const min = Math.floor((Date.now() - ts) / 60000);
    if (min < 1) return 'now';
    if (min < 60) return `${min} min`;
    if (dayKey(ts) === dayKey(Date.now())) return clock(ts);
    if (isYesterday(ts)) return 'Yesterday';
    return new Date(ts).toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
  }
  function fmtSize(bytes) {
    const units = ['B', 'KB', 'MB', 'GB'];
    let v = bytes;
    let i = 0;
    while (v >= 1024 && i < units.length - 1) {
      v /= 1024;
      i++;
    }
    return `${i === 0 ? v : v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
  }
  const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;

  // ------------------------------------------------------------------ state

  const state = {
    mode: 'connected',
    computer: null,
    device: null,
    devices: [],
    serviceDown: false,
    tab: 'notifications',
    notifications: [],
    threads: [],
    icons: {}, // package → data: URL
    transfers: [],
    settings: null,
    activeThread: null,
    threadSearch: '',
    drafts: new Map(),
    replyOpen: null,
    replyDrafts: new Map(),
    replied: new Set(),
    busy: new Set(),
    showPairing: false,
    pairing: null, // { expiresAt, cancellable, qr, id, loading, error }
  };
  let loadSeq = 0;
  // Every notification-list request is numbered; an older answer never overwrites a newer
  // one (a slow full reload must not undo a live update that finished first).
  let notifRequested = 0;
  let notifApplied = 0;
  function fetchNotifications() {
    const seq = ++notifRequested;
    return api.notifications().then((list) => ({ seq, list }));
  }
  function applyNotifications({ seq, list }) {
    if (seq < notifApplied) return false;
    notifApplied = seq;
    state.notifications = list.sort((a, b) => b.time - a.time);
    if (state.replyOpen && !state.notifications.some((n) => n.id === state.replyOpen)) state.replyOpen = null;
    return true;
  }
  let tempSeq = 0;

  const el = {
    win: $('#window'),
    pill: $('#conn-pill'),
    body: $('#main-body'),
    device: $('#device-panel'),
    banner: $('#offline-banner'),
    tabs: {
      notifications: $('#tab-notifications'),
      messages: $('#tab-messages'),
      files: $('#tab-files'),
    },
    panels: {
      notifications: $('#panel-notifications'),
      messages: $('#panel-messages'),
      files: $('#panel-files'),
    },
    threadList: $('#thread-list'),
    threadSearch: $('#thread-search'),
    convHeader: $('#conv-header'),
    convScroll: $('#conv-scroll'),
    composer: $('#composer'),
    composerInput: $('#composer-input'),
    composerSend: $('#composer-send'),
    pairing: $('#pairing'),
    service: $('#service'),
    toast: $('#toast'),
    fileInput: $('#file-input'),
  };

  // ------------------------------------------------------------------ feedback

  let toastTimer = 0;
  function toast(text) {
    clearTimeout(toastTimer);
    el.toast.hidden = true;
    void el.toast.offsetWidth; // restart the entry animation
    el.toast.textContent = text;
    el.toast.hidden = false;
    toastTimer = setTimeout(() => {
      el.toast.hidden = true;
    }, 2800);
  }

  function run(fn) {
    return (...args) =>
      Promise.resolve()
        .then(() => fn(...args))
        .catch((err) => {
          console.warn('PhoneLink UI:', err);
          toast(`Something went wrong: ${err && err.message ? err.message : err}`);
        });
  }

  const deviceName = () => (state.device ? state.device.name : 'your phone');

  // ------------------------------------------------------------------ loading

  async function load() {
    const seq = ++loadSeq;
    let st;
    try {
      st = await api.status();
    } catch (err) {
      if (err && err.code === 'daemon-down') {
        if (seq !== loadSeq) return;
        state.serviceDown = true;
        renderChrome();
        renderService();
        return;
      }
      throw err;
    }
    state.serviceDown = false;
    const [notifications, threads, transfers, settings] = await Promise.all([
      // A failing list (e.g. an older service) must not stop the rest of the app loading.
      fetchNotifications().catch((err) => {
        console.info('PhoneLink UI: notifications unavailable:', err && err.message);
        return null;
      }),
      api.threads(),
      api.transfers(),
      api.settings(),
    ]);
    if (seq !== loadSeq) return; // a newer load superseded this one
    state.mode = st.mode;
    requestedMode = st.mode;
    state.computer = st.computer;
    state.device = st.device;
    state.devices = st.devices || (st.device ? [st.device] : []);
    if (notifications) applyNotifications(notifications);
    state.threads = threads;
    state.transfers = transfers;
    state.settings = settings;
    if (api.appIcons) state.icons = (await api.appIcons().catch(() => ({}))) || {};
    if (seq !== loadSeq) return;
    if (!threads.some((t) => t.id === state.activeThread)) {
      state.activeThread = threads.length ? sortedThreads()[0].id : null;
    }
    if (!state.device) {
      if (!state.showPairing) startPairing(false);
      else if (state.pairing) state.pairing.cancellable = false;
    } else if (state.showPairing && !state.pairing?.cancellable) {
      state.showPairing = false;
      state.pairing = null;
    }
    renderAll();
    if (state.tab === 'messages') markActiveRead();
  }

  // ------------------------------------------------------------------ rendering

  function renderAll() {
    renderChrome();
    renderDevice();
    renderTabs();
    renderNotifications();
    renderThreads();
    renderConversation();
    renderFiles();
    if (state.showPairing) renderPairing();
  }

  function renderChrome() {
    const d = state.device;
    if (!d) {
      el.pill.innerHTML = '<span class="dot"></span>No phone paired';
    } else if (d.connected) {
      el.pill.innerHTML = `<span class="dot ok"></span>${esc(d.name)} · Connected`;
    } else {
      el.pill.innerHTML = `<span class="dot warn"></span>${esc(d.name)} · Offline`;
    }
    for (const b of document.querySelectorAll('[data-preview]')) {
      b.setAttribute('aria-pressed', String(b.dataset.preview === state.mode));
    }
    const view = state.serviceDown ? 'service' : state.showPairing ? 'pairing' : 'main';
    el.body.hidden = view !== 'main';
    el.pairing.hidden = view !== 'pairing';
    el.service.hidden = view !== 'service';
    if (view === 'service') {
      el.pill.innerHTML = '<span class="dot warn"></span>Service not running';
    }
    const gear = $('.titlebar [data-action="open-settings"]');
    gear.disabled = view === 'service';
    gear.title = gear.disabled ? 'Settings are available once the service is running' : 'Settings';

    const offline = view === 'main' && d && !d.connected;
    el.banner.hidden = !offline;
    if (offline) {
      el.banner.innerHTML = `${icon('alert')}<span class="grow"><strong>${esc(d.name)} is offline.</strong> Messages and files will be sent when it reconnects.</span><button type="button" class="link-btn" data-action="troubleshoot">Troubleshoot</button>`;
    }
  }

  function renderDevice() {
    const d = state.device;
    if (!d) {
      el.device.innerHTML = '';
      return;
    }
    const now = new Date();
    const date = now.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' });
    const hasBattery = typeof d.battery === 'number';
    const battWidth = hasBattery ? Math.round((14 * d.battery) / 100) : 0;
    const statusRows = d.connected
      ? `<li class="stat"><span class="dot ok"></span><span>Connected</span></li>
         ${d.network ? `<li class="stat">${icon('wifi')}<span>${esc(d.network)}</span></li>` : ''}
         ${hasBattery ? `<li class="stat"><span class="battery${d.battery < 20 ? ' low' : ''}" aria-hidden="true"><i style="width:${battWidth}px"></i></span><span class="tnum">${d.battery}%${d.charging ? ' · charging' : ''}</span></li>` : ''}
         ${d.pending ? `<li class="stat faint">${icon('send')}<span>${esc(plural(d.pending, 'message', 'messages'))} sending…</span></li>` : ''}`
      : `<li class="stat"><span class="dot warn"></span><span>Offline${d.lastSeen ? ` · last seen <span data-ts="${d.lastSeen}" data-rel="ago">${esc(relTime(d.lastSeen))}</span>` : ''}</span></li>
         ${caps.deviceDetails ? `<li class="stat faint">${icon('wifi')}<span>Not on this network</span></li>` : ''}
         ${d.pending ? `<li class="stat faint">${icon('send')}<span>${esc(plural(d.pending, 'message', 'messages'))} waiting</span></li>` : ''}`;
    const clipOn = !!state.settings?.clipboardSync;
    el.device.innerHTML = `
      <div class="phone-art${d.connected ? '' : ' off'}" aria-hidden="true">
        <div class="clock tnum" data-clock>${esc(clock(now))}</div>
        <div class="date">${esc(date)}</div>
      </div>
      <div class="device-head">
        <div class="device-name" title="${esc(d.name)}">${esc(d.name)}</div>
        <div class="device-model">${esc(d.model || 'Paired phone')}</div>
      </div>
      <ul class="stats">${statusRows}</ul>
      <div>
        <p class="section-label">Quick actions</p>
        <div class="actions">
          <button type="button" class="action" data-action="send-note">${icon('send')}<span class="grow">Send a note</span></button>
          ${caps.files ? `<button type="button" class="action" data-action="send-file">${icon('upload')}<span class="grow">Send files</span></button>` : ''}
          ${caps.clipboard ? `<button type="button" class="action" role="switch" aria-checked="${clipOn}" data-action="toggle-clipboard">${icon('clipboard')}<span class="grow">Sync clipboard</span><span class="switch" aria-hidden="true"></span></button>` : ''}
        </div>
      </div>
      <div class="devices-foot">
        <p class="section-label">Devices</p>
        ${state.devices
          .map((x) => `<div class="dev-row">${icon('phone')}<span class="grow" title="${esc(x.name)}">${esc(x.name)}</span><span class="dot ${x.connected ? 'ok' : 'warn'}"></span></div>`)
          .join('')}
        <button type="button" class="link-btn" data-action="pair-new">Pair another device</button>
      </div>`;
  }

  function renderTabs() {
    const unread = state.threads.reduce((n, t) => n + (t.unread || 0), 0);
    const active = state.transfers.filter((t) => t.state === 'active' || t.state === 'paused').length;
    const defs = {
      notifications: { icon: 'bell', label: 'Notifications', badge: state.notifications.length },
      messages: { icon: 'chat', label: 'Messages', badge: unread },
      files: { icon: 'folder', label: 'Files', badge: active },
    };
    for (const [name, tab] of Object.entries(el.tabs)) {
      const def = defs[name];
      const selected = state.tab === name;
      tab.innerHTML = `${icon(def.icon)}<span>${def.label}</span>${def.badge ? `<span class="badge tnum">${def.badge}</span>` : ''}`;
      tab.setAttribute('aria-selected', String(selected));
      tab.tabIndex = selected ? 0 : -1;
      el.panels[name].hidden = !selected;
    }
  }

  function renderNotifications() {
    const list = state.notifications;
    if (!list.length) {
      const hint = caps.notificationMirroring
        ? `New notifications from ${esc(deviceName())} appear here.`
        : `Notes you send from PhoneLink on ${esc(deviceName())} appear here.<br>Showing your phone's app notifications (WhatsApp, Gmail…) comes in the next update.`;
      el.panels.notifications.innerHTML = `<div class="empty">${icon('bell')}<strong>You're all caught up</strong><span>${hint}</span></div>`;
      return;
    }
    // Keep keyboard focus on the same control across live updates.
    const focused = document.activeElement && document.activeElement.closest('.notif [data-action]');
    const keep = focused
      ? { id: focused.closest('.notif').dataset.id, action: focused.dataset.action, index: focused.dataset.index }
      : null;
    const groups = new Map();
    for (const n of list) {
      if (!groups.has(n.app)) groups.set(n.app, []);
      groups.get(n.app).push(n);
    }
    let html = `<div class="notif-wrap">
      <div class="toolbar"><span>${esc(plural(list.length, 'notification', 'notifications'))}</span>
        <button type="button" class="btn btn-ghost btn-sm" data-action="clear-all">Clear all</button></div>`;
    for (const [app, items] of groups) {
      const iconUrl = items[0].package && state.icons[items[0].package];
      const badge = iconUrl && /^data:image\/png;base64,[A-Za-z0-9+/=]+$/.test(iconUrl)
        ? `<img class="app-badge app-icon" src="${iconUrl}" alt="" aria-hidden="true">`
        : `<span class="app-badge" style="--c:${colorFor(app)}" aria-hidden="true">${esc(initial(app))}</span>`;
      html += `<section class="notif-group" aria-label="${esc(app)}">
        <div class="group-head">${badge}${esc(app)}</div>`;
      for (const n of items) {
        const replying = state.replyOpen === n.id;
        html += `<article class="notif" data-id="${esc(n.id)}">
          <div class="notif-title" title="${esc(n.title)}">${esc(n.title)}</div>
          <time class="notif-time tnum" data-ts="${n.time}">${esc(relTime(n.time))}</time>
          <button type="button" class="icon-btn icon-btn-sm" data-action="dismiss" aria-label="Dismiss notification: ${esc(n.title)}" title="Dismiss">${icon('close')}</button>
          <div class="notif-text">${esc(n.text)}</div>
          ${(() => {
            const buttons = caps.notificationButtons && !n.removedOnPhone ? n.actions || [] : [];
            const parts = [
              n.canReply && !replying && !n.removedOnPhone ? `<button type="button" class="btn btn-sm" data-action="reply-open">${icon('reply')}Reply</button>` : '',
              ...buttons.map((label, i) => `<button type="button" class="btn btn-ghost btn-sm" data-action="notif-button" data-index="${i}">${esc(label)}</button>`),
              state.replied.has(n.id) ? `<span class="notif-note">${icon('check')}Replied</span>` : '',
              n.removedOnPhone ? '<span class="notif-note faint">Cleared on phone</span>' : '',
            ].filter(Boolean);
            return parts.length ? `<div class="notif-actions">${parts.join('')}</div>` : '';
          })()}
          ${replying ? `<form class="reply-form" data-form="reply">
              <input class="text-input" name="reply" maxlength="2000" placeholder="Reply to ${esc(n.title)}" aria-label="Reply to ${esc(n.title)}">
              <button type="submit" class="btn btn-primary btn-sm">Send</button>
              <button type="button" class="btn btn-ghost btn-sm" data-action="reply-cancel">Cancel</button>
            </form>` : ''}
        </article>`;
      }
      html += '</section>';
    }
    html += '</div>';
    el.panels.notifications.innerHTML = html;
    if (keep && !state.replyOpen) {
      const sel = `.notif[data-id="${CSS.escape(keep.id)}"] [data-action="${CSS.escape(keep.action)}"]` +
        (keep.index !== undefined ? `[data-index="${CSS.escape(keep.index)}"]` : '');
      const again = $(sel, el.panels.notifications);
      if (again) again.focus();
    }
    if (state.replyOpen) {
      const input = $(`.notif[data-id="${CSS.escape(state.replyOpen)}"] input[name="reply"]`, el.panels.notifications);
      if (input) {
        input.value = state.replyDrafts.get(state.replyOpen) || '';
        input.focus();
      }
    }
  }

  function lastMessage(t) {
    return t.messages[t.messages.length - 1];
  }
  function sortedThreads() {
    return [...state.threads].sort((a, b) => (lastMessage(b)?.time || 0) - (lastMessage(a)?.time || 0));
  }

  function renderThreads() {
    el.panels.messages.classList.toggle('unavailable', !caps.sms);
    if (!caps.sms) return;
    const q = state.threadSearch.trim().toLowerCase();
    const list = sortedThreads().filter(
      (t) => !q || t.name.toLowerCase().includes(q) || t.messages.some((m) => m.text.toLowerCase().includes(q)),
    );
    if (!list.length) {
      el.threadList.innerHTML = `<li class="thread-empty">${q ? `No conversations match “${esc(state.threadSearch.trim())}”` : 'No conversations yet'}</li>`;
      return;
    }
    el.threadList.innerHTML = list
      .map((t) => {
        const last = lastMessage(t);
        const preview = last ? `${last.from === 'me' ? 'You: ' : ''}${last.text}` : '';
        const current = t.id === state.activeThread;
        return `<li><button type="button" class="thread${t.unread ? ' unread' : ''}" data-thread="${esc(t.id)}" aria-current="${current}">
          <span class="avatar" style="--c:${colorFor(t.name)}" aria-hidden="true">${esc(initial(t.name))}</span>
          <span class="thread-name" title="${esc(t.name)}">${esc(t.name)}</span>
          <span class="thread-time tnum">${last ? esc(relTime(last.time)) : ''}</span>
          <span class="thread-last">${esc(preview)}</span>
          ${t.unread ? '<span class="unread-dot" aria-label="Unread"></span>' : '<span></span>'}
        </button></li>`;
      })
      .join('');
  }

  function activeThread() {
    return state.threads.find((t) => t.id === state.activeThread) || null;
  }

  function renderConversation() {
    const t = activeThread();
    el.composerInput.disabled = !t || !state.device;
    if (!t) {
      el.convHeader.innerHTML = '';
      el.convScroll.innerHTML = `<div class="empty">${icon('chat')}<strong>No conversation selected</strong></div>`;
      updateComposer();
      return;
    }
    el.convHeader.innerHTML = `<span class="avatar" style="--c:${colorFor(t.name)}" aria-hidden="true">${esc(initial(t.name))}</span>
      <div class="conv-title"><strong title="${esc(t.name)}">${esc(t.name)}</strong><span>${esc(t.number)}</span></div>`;

    let html = '';
    let lastDay = '';
    const msgs = t.messages;
    msgs.forEach((m, i) => {
      const day = dayKey(m.time);
      if (day !== lastDay) {
        html += `<div class="day-sep">${esc(dayLabel(m.time))}</div>`;
        lastDay = day;
      }
      const next = msgs[i + 1];
      const endOfGroup = !next || next.from !== m.from || next.time - m.time > 5 * 60000 || dayKey(next.time) !== day;
      const pending = m.status === 'sending';
      html += `<div class="bubble ${m.from}${pending ? ' pending' : ''}">${esc(m.text)}</div>`;
      if (endOfGroup) {
        let meta = clock(m.time);
        if (m.from === 'me') {
          meta += {
            sending: ' · Sending…',
            delivered: ' · Delivered',
            queued: ' · Queued until your phone reconnects',
            failed: ' · Not sent',
          }[m.status] || '';
        }
        html += `<div class="bubble-meta ${m.from} tnum">${esc(meta)}</div>`;
      }
    });
    el.convScroll.innerHTML = html;
    el.convScroll.scrollTop = el.convScroll.scrollHeight;
    el.composerInput.placeholder = `Text ${t.name}`;
    updateComposer();
  }

  function updateComposer() {
    const input = el.composerInput;
    input.style.height = 'auto';
    input.style.height = `${Math.min(input.scrollHeight + 2, 120)}px`;
    el.composerSend.disabled = input.disabled || !input.value.trim();
  }

  function transferStatus(t) {
    const pct = Math.floor(t.progress * 100);
    const toPhone = t.direction === 'to-phone';
    if (t.state === 'done') return toPhone ? 'Sent to phone' : 'Saved to Downloads';
    if (t.state === 'paused') return `Paused at ${pct}% · resumes when your phone reconnects`;
    return `${toPhone ? 'Sending' : 'Receiving'} · ${pct}%`;
  }

  function renderFiles() {
    if (!caps.files) {
      el.panels.files.innerHTML = `<div class="empty">${icon('folder')}<strong>File transfer is coming in the next update</strong><span>You'll be able to drop files here to send them to ${esc(deviceName())}, with transfers that resume if the connection drops.</span></div>`;
      return;
    }
    const items = state.transfers
      .map((t) => {
        const side =
          t.state === 'done'
            ? t.direction === 'from-phone'
              ? `<button type="button" class="btn btn-sm" data-action="show-in-folder">Show in folder</button>`
              : `<span class="state-ok">${icon('check')}Done</span>`
            : `<button type="button" class="btn btn-ghost btn-sm" data-action="cancel-transfer" aria-label="Cancel ${esc(t.name)}">Cancel</button>`;
        const pct = Math.floor(t.progress * 100);
        return `<div class="transfer ${t.state}" data-id="${esc(t.id)}">
          <div class="file-ico">${icon(t.direction === 'to-phone' ? 'upload' : 'download')}</div>
          <div class="transfer-name" title="${esc(t.name)}">${esc(t.name)}</div>
          <div class="transfer-side">${side}</div>
          <div class="transfer-meta tnum" data-meta>${esc(fmtSize(t.size))} · ${esc(transferStatus(t))}</div>
          ${t.state === 'done' ? '' : `<div class="progress" role="progressbar" aria-label="${esc(t.name)}" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${pct}"><i style="width:${pct}%"></i></div>`}
        </div>`;
      })
      .join('');
    el.panels.files.innerHTML = `<div class="files-wrap">
      <div class="dropzone">${icon('upload')}
        <div class="grow"><strong>Drop files here to send them to ${esc(deviceName())}</strong>
          <span>Transfers pick up where they left off if the connection drops.</span></div>
        <button type="button" class="btn" data-action="send-file">Choose files</button>
      </div>
      ${items ? `<p class="section-label">Transfers</p>${items}` : `<div class="empty">${icon('folder')}<strong>No transfers yet</strong><span>Files you send or receive appear here.</span></div>`}
    </div>`;
  }

  function renderPairing() {
    const p = state.pairing;
    if (!p) return;
    const left = p.loading || p.error ? 0 : Math.max(0, Math.ceil((p.expiresAt - Date.now()) / 1000));
    const expired = !p.loading && !p.error && left === 0;
    let qrInner;
    if (p.loading) {
      qrInner = '<div class="qr-expired">Preparing a code…</div>';
    } else if (p.error) {
      qrInner = `<div class="qr-expired"><span>${esc(p.error)}</span><button type="button" class="btn btn-primary btn-sm" data-action="new-code">Try again</button></div>`;
    } else {
      qrInner = `<svg viewBox="0 0 ${p.qr.size} ${p.qr.size}" role="img" aria-label="Pairing code to scan with your phone"><path fill="#111417" d="${esc(p.qr.path)}"/></svg>
        ${expired ? '<div class="qr-expired">This code expired<button type="button" class="btn btn-primary btn-sm" data-action="new-code">Show a new code</button></div>' : ''}`;
    }
    el.pairing.innerHTML = `<div class="pair-card">
      <div class="qr-box${expired ? ' expired' : ''}">${qrInner}</div>
      <div class="pair-text">
        <h1>Pair your phone</h1>
        <p>Your phone and this computer must be on the same Wi&#8209;Fi, or this computer on your phone's hotspot.</p>
        <ol class="steps">
          <li><span>Install <strong>PhoneLink</strong> on your Android phone.</span></li>
          <li><span>Open it and tap <strong>Pair a computer</strong>.</span></li>
          <li><span>Scan this code. Check that the phone shows the same id as below.</span></li>
        </ol>
        <div class="id-box"><span class="muted">This computer</span><span class="mono">${esc(p.id || (state.computer ? state.computer.id : ''))}</span></div>
        <div class="pair-foot">
          <span class="tnum" data-countdown>${p.loading || p.error ? '' : expired ? 'Code expired' : `Expires in ${Math.floor(left / 60)}:${pad(left % 60)} · single use`}</span>
          ${p.cancellable ? '<button type="button" class="btn btn-sm" data-action="cancel-pairing">Cancel</button>' : ''}
          ${caps.preview && !p.loading && !p.error ? '<button type="button" class="link-btn" data-action="simulate-pair">Simulate a scan (preview)</button>' : ''}
        </div>
      </div>
    </div>`;
  }

  // ------------------------------------------------------------------ tabs & threads

  function selectTab(name, focus = false) {
    if (!el.tabs[name] || state.showPairing) return;
    state.tab = name;
    renderTabs();
    if (focus) el.tabs[name].focus();
    if (name === 'messages') markActiveRead();
  }

  function markActiveRead() {
    const t = activeThread();
    if (t && t.unread) {
      t.unread = 0;
      api.markRead(t.id).catch(() => {});
      renderThreads();
      renderTabs();
    }
  }

  function openThread(id) {
    if (id === state.activeThread) return;
    state.drafts.set(state.activeThread, el.composerInput.value);
    state.activeThread = id;
    el.composerInput.value = state.drafts.get(id) || '';
    renderThreads();
    renderConversation();
    markActiveRead();
    if (!el.composerInput.disabled) el.composerInput.focus();
  }

  async function sendSms() {
    const t = activeThread();
    const text = el.composerInput.value.replace(/\s+$/, '');
    if (!t || !text.trim() || !state.device) return;
    const temp = { id: `tmp-${++tempSeq}`, from: 'me', text, time: Date.now(), status: 'sending' };
    t.messages.push(temp);
    el.composerInput.value = '';
    state.drafts.delete(t.id);
    renderConversation();
    renderThreads();
    let result;
    try {
      result = await api.sendSms(t.id, text);
    } catch (err) {
      result = null;
      toast(`Message not sent: ${err.message || err}`);
    }
    // The thread list may have been reloaded meanwhile: look the thread up again.
    const thread = state.threads.find((x) => x.id === t.id);
    if (!thread) return;
    const idx = thread.messages.findIndex((m) => m.id === temp.id);
    if (result) {
      if (idx >= 0) thread.messages[idx] = result;
      else if (!thread.messages.some((m) => m.id === result.id)) thread.messages.push(result);
      if (result.status === 'queued') toast(`Queued. It will be sent when ${deviceName()} reconnects.`);
    } else if (idx >= 0) {
      thread.messages[idx] = { ...temp, status: 'failed' };
    }
    if (state.activeThread === thread.id) renderConversation();
    renderThreads();
  }

  // ------------------------------------------------------------------ notifications

  async function submitReply(form) {
    const article = form.closest('.notif');
    const id = article && article.dataset.id;
    const n = state.notifications.find((x) => x.id === id);
    const input = form.elements.reply;
    const text = input.value.trim();
    if (!n || !text) {
      input.focus();
      return;
    }
    if (state.busy.has(id)) return;
    state.busy.add(id);
    for (const c of form.elements) c.disabled = true;
    try {
      const res = await api.reply(id, text);
      state.replied.add(id);
      state.replyOpen = null;
      state.replyDrafts.delete(id);
      toast(res.queued ? `Reply queued. It will be sent when ${deviceName()} reconnects.` : `Reply sent to ${n.title}`);
    } finally {
      state.busy.delete(id);
      renderNotifications();
      const btn = $(`.notif[data-id="${CSS.escape(id)}"] [data-action="dismiss"]`, el.panels.notifications);
      if (btn) btn.focus();
    }
  }

  async function dismiss(id) {
    if (state.busy.has(id)) return;
    const article = $(`.notif[data-id="${CSS.escape(id)}"]`, el.panels.notifications);
    const hadFocus = article && article.contains(document.activeElement);
    const order = state.notifications.map((n) => n.id);
    state.busy.add(id);
    if (article) article.classList.add('leaving');
    try {
      await Promise.all([api.dismiss(id), wait(140)]);
      state.notifications = state.notifications.filter((n) => n.id !== id);
      if (state.replyOpen === id) state.replyOpen = null;
    } finally {
      state.busy.delete(id);
      renderNotifications();
      renderTabs();
    }
    if (hadFocus) {
      const nextId = order[order.indexOf(id) + 1] || order[order.indexOf(id) - 1];
      const target = nextId && $(`.notif[data-id="${CSS.escape(nextId)}"] [data-action="dismiss"]`, el.panels.notifications);
      (target || el.tabs.notifications).focus();
    }
  }

  // ------------------------------------------------------------------ files

  async function sendFiles(files) {
    if (!files.length) return;
    if (!caps.files) {
      toast('File transfer is coming in the next update.');
      return;
    }
    if (!state.device) {
      toast('Pair your phone first.');
      return;
    }
    const added = await api.sendFiles(files.map((f) => ({ name: f.name, size: f.size })));
    state.transfers.unshift(...added);
    selectTab('files');
    renderFiles();
    renderTabs();
    const what = files.length === 1 ? `“${files[0].name}”` : plural(files.length, 'file', 'files');
    toast(state.device.connected ? `Sending ${what} to ${deviceName()}` : `${what} queued until ${deviceName()} reconnects`);
  }

  const SPEED = 30 * 1024 * 1024; // simulated bytes per second
  function tickTransfers() {
    if (!state.device || !state.device.connected) return;
    let finished = false;
    for (const t of state.transfers) {
      if (t.state !== 'active') continue;
      t.progress = Math.min(1, t.progress + Math.max(0.02, (SPEED * 0.5) / Math.max(1, t.size)));
      if (t.progress >= 1) {
        t.state = 'done';
        finished = true;
        toast(`${t.direction === 'to-phone' ? 'Sent' : 'Received'} “${t.name}”`);
      }
      api.transferProgress(t.id, t.progress, t.state).catch(() => {});
      const row = $(`.transfer[data-id="${CSS.escape(t.id)}"]`, el.panels.files);
      if (row && !finished) {
        const pct = Math.floor(t.progress * 100);
        const bar = $('.progress', row);
        if (bar) {
          bar.setAttribute('aria-valuenow', String(pct));
          bar.firstElementChild.style.width = `${pct}%`;
        }
        $('[data-meta]', row).textContent = `${fmtSize(t.size)} · ${transferStatus(t)}`;
      }
    }
    if (finished) {
      const hadFocus = el.panels.files.contains(document.activeElement);
      renderFiles();
      renderTabs();
      if (hadFocus) el.tabs.files.focus();
    }
  }

  // ------------------------------------------------------------------ dialogs

  let dialog = null;
  const FOCUSABLE = 'button:not([disabled]), input:not([disabled]), textarea:not([disabled]), [href], [tabindex]:not([tabindex="-1"])';
  const focusables = (root) => [...root.querySelectorAll(FOCUSABLE)].filter((x) => x.offsetParent !== null);

  function openDialog(html, { sheet = false } = {}) {
    closeDialog(false);
    const back = document.createElement('div');
    back.className = sheet ? 'backdrop' : 'backdrop center';
    back.innerHTML = html;
    el.win.appendChild(back);
    dialog = { el: back, prevFocus: document.activeElement };
    back.addEventListener('mousedown', (e) => {
      if (e.target === back) closeDialog();
    });
    const first = back.querySelector('textarea, input') || focusables(back)[0];
    if (first) first.focus();
    return back;
  }

  function closeDialog(restore = true) {
    if (!dialog) return;
    const { el: node, prevFocus } = dialog;
    dialog = null;
    node.remove();
    if (restore && prevFocus && document.contains(prevFocus)) prevFocus.focus();
  }

  function openNoteDialog() {
    const node = openDialog(`<div class="dialog" role="dialog" aria-modal="true" aria-labelledby="note-title">
      <form data-form="note">
        <div class="dialog-body">
          <h2 id="note-title">Send a note to ${esc(deviceName())}</h2>
          <p>It appears as a notification on your phone.</p>
          <textarea name="note" maxlength="4000" aria-label="Note" placeholder="Type a note"></textarea>
        </div>
        <div class="dialog-foot">
          <button type="button" class="btn" data-action="close-dialog">Cancel</button>
          <button type="submit" class="btn btn-primary" disabled>Send</button>
        </div>
      </form></div>`);
    const area = node.querySelector('textarea');
    const submit = node.querySelector('[type="submit"]');
    area.addEventListener('input', () => {
      submit.disabled = !area.value.trim();
    });
    area.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        if (!submit.disabled) node.querySelector('form').requestSubmit();
      }
    });
  }

  async function submitNote(form) {
    const text = form.elements.note.value.trim();
    if (!text) return;
    for (const c of form.elements) c.disabled = true;
    try {
      const res = await api.sendText(text, state.device ? state.device.id : null);
      closeDialog();
      toast(res.queued ? `Queued. It will be delivered when ${deviceName()} reconnects.` : `Sent to ${deviceName()}`);
    } catch (err) {
      for (const c of form.elements) c.disabled = false;
      throw err;
    }
  }

  function openTroubleshoot() {
    openDialog(`<div class="dialog" role="dialog" aria-modal="true" aria-labelledby="tips-title">
      <div class="dialog-body">
        <h2 id="tips-title">Can't reach ${esc(deviceName())}</h2>
        <ol class="tips">
          <li><strong>Same network?</strong> The phone must be on the same Wi&#8209;Fi, not mobile data or a guest network.</li>
          <li><strong>College or office Wi&#8209;Fi</strong> often blocks devices from reaching each other. Turn on your phone's hotspot and connect this computer to it.</li>
          <li><strong>VPN?</strong> Allow local network access in the VPN app, or pause it.</li>
          <li><strong>Battery saving.</strong> On the phone, add PhoneLink to Settings → Battery → Background usage limits → Never sleeping apps.</li>
          <li><strong>Firewall.</strong> If ufw is on, allow UDP port 47100 from your local network.</li>
        </ol>
      </div>
      <div class="dialog-foot"><button type="button" class="btn btn-primary" data-action="close-dialog">Close</button></div>
    </div>`);
  }

  function switchRow(label, desc, action, checked, extra = '') {
    return `<div class="row"><div class="grow"><div>${esc(label)}</div>${desc ? `<div class="row-desc">${esc(desc)}</div>` : ''}</div>
      <button type="button" class="switch-btn" role="switch" aria-checked="${checked}" aria-label="${esc(label)}" data-action="${action}" ${extra}><span class="switch" aria-hidden="true"></span></button></div>`;
  }

  function settingsBody() {
    const s = state.settings || { apps: [] };
    const deviceRow = state.devices.length
      ? state.devices
          .map(
            (d) => `<div class="row" data-device-id="${esc(d.id)}">
          <span class="dot ${d.connected ? 'ok' : 'warn'}"></span>
          <div class="grow"><div>${esc(d.name)}</div><div class="row-desc mono">${esc(d.id)}</div></div>
          <span data-unpair-slot><button type="button" class="btn btn-danger btn-sm" data-action="unpair-ask" aria-label="Unpair ${esc(d.name)}">Unpair</button></span>
        </div>`,
          )
          .join('')
      : '<div class="row faint">No devices paired.</div>';
    return `
      <div class="sheet-section"><p class="section-label">This computer</p>
        <div class="row"><div class="grow"><div>${esc(state.computer ? state.computer.name : '')}</div>
          <div class="row-desc mono">${esc(state.computer ? state.computer.id : '')}</div></div>
          <button type="button" class="btn btn-sm" data-action="copy-id">Copy id</button></div>
      </div>
      <div class="sheet-section"><p class="section-label">Paired devices</p>${deviceRow}</div>
      ${caps.appFilters && s.apps.length ? `<div class="sheet-section"><p class="section-label">Show notifications from</p>
        ${s.apps.map((a) => switchRow(a.name, '', 'toggle-app', a.enabled, `data-name="${esc(a.name)}"`)).join('')}
      </div>` : ''}
      <div class="sheet-section"><p class="section-label">General</p>
        ${switchRow('Desktop notifications', 'Show messages from your phone as system notifications.', 'toggle-setting', !!s.desktopNotifications, 'data-key="desktopNotifications"')}
        ${caps.syncDismissals ? switchRow('Dismiss on both devices', 'Clearing a notification here also clears it on your phone, and the other way round.', 'toggle-setting', !!s.syncDismissals, 'data-key="syncDismissals"') : ''}
        ${caps.discoveryToggle ? switchRow('Find my phone on this network', 'Anonymous: nothing that identifies you is broadcast.', 'toggle-setting', !!s.discovery, 'data-key="discovery"') : ''}
      </div>`;
  }

  async function openSettings() {
    // Fresh every time: apps that started sending notifications since startup must appear.
    try {
      state.settings = await api.settings();
    } catch (err) {
      console.info('PhoneLink UI: using cached settings:', err && err.message);
    }
    openDialog(`<section class="sheet" role="dialog" aria-modal="true" aria-labelledby="settings-title">
      <header class="sheet-head"><h2 id="settings-title">Settings</h2>
        <button type="button" class="icon-btn" data-action="close-dialog" aria-label="Close settings">${icon('close')}</button></header>
      <div class="sheet-body">${settingsBody()}</div>
    </section>`, { sheet: true });
  }

  function setSwitch(btn, on) {
    btn.setAttribute('aria-checked', String(on));
  }

  // ------------------------------------------------------------------ pairing

  let pairingSeq = 0;
  async function startPairing(cancellable) {
    const seq = ++pairingSeq;
    state.showPairing = true;
    state.pairing = { loading: true, cancellable, expiresAt: 0 };
    state.replyOpen = null;
    renderChrome();
    renderPairing();
    try {
      const code = await api.startPairing();
      if (seq !== pairingSeq || !state.showPairing) return;
      state.pairing = { cancellable: state.pairing.cancellable, qr: code.qr, id: code.id, expiresAt: Date.now() + code.expiresIn * 1000 };
    } catch (err) {
      if (seq !== pairingSeq || !state.showPairing) return;
      if (err && err.code === 'daemon-down') {
        state.serviceDown = true;
        renderChrome();
        renderService();
        return;
      }
      state.pairing = { cancellable: state.pairing.cancellable, error: `Couldn't create a code: ${err.message || err}`, expiresAt: 0 };
    }
    renderPairing();
  }

  function stopPairing() {
    pairingSeq++;
    state.showPairing = false;
    state.pairing = null;
    api.cancelPairing().catch(() => {});
    renderChrome();
  }

  api.onPairing((ev) => {
    if (!state.showPairing) return;
    if (ev.kind === 'paired') {
      pairingSeq++;
      state.showPairing = false;
      state.pairing = null;
      toast(`Paired with ${ev.name}`);
      run(load)();
    } else if (ev.kind === 'expired' && state.pairing && !state.pairing.loading) {
      state.pairing.expiresAt = Date.now();
      renderPairing();
    } else if (ev.kind === 'failed' && state.pairing) {
      state.pairing = { cancellable: state.pairing.cancellable, error: ev.message, expiresAt: 0 };
      renderPairing();
    }
  });

  // ------------------------------------------------------------------ service not running

  function renderService() {
    el.service.innerHTML = `<div class="service-card">
      ${icon('alert')}
      <h1>The PhoneLink service isn't running</h1>
      <p>It runs in the background and keeps your phone connected, even when this window is closed.</p>
      <div class="service-actions">
        <button type="button" class="btn btn-primary" data-action="start-daemon">Start the service</button>
      </div>
      <p class="faint">Or in a terminal: <span class="mono">systemctl --user start phonelinkd</span></p>
    </div>`;
  }

  // Live updates from the service (inside the desktop app).
  let reloadTimer = 0;
  const scheduleLoad = () => {
    clearTimeout(reloadTimer);
    reloadTimer = setTimeout(run(load), 150);
  };
  let notifTimer = 0;
  const refreshNotifications = () => {
    clearTimeout(notifTimer);
    notifTimer = setTimeout(
      run(async () => {
        if (!applyNotifications(await fetchNotifications())) return;
        renderNotifications();
        renderTabs();
      }),
      80,
    );
  };
  const NOTIFICATION_EVENTS = ['text', 'notification_posted', 'notification_removed', 'notifications_changed'];
  api.onEvent((ev) => {
    if (NOTIFICATION_EVENTS.includes(ev.kind)) return refreshNotifications();
    if (ev.kind === 'action_result') {
      if (!ev.ok) toast(`Your phone couldn't do that: ${ev.message}`);
      return undefined;
    }
    if (ev.kind === 'app_icon') {
      if (api.appIcons) {
        api.appIcons().then((icons) => {
          state.icons = icons || {};
          renderNotifications();
        }).catch(() => {});
      }
      return undefined;
    }
    return scheduleLoad();
  });
  api.onDaemon((up) => {
    if (up) scheduleLoad();
    else if (!state.serviceDown) {
      state.serviceDown = true;
      closeDialog(false);
      renderChrome();
      renderService();
    }
  });

  // ------------------------------------------------------------------ preview controls

  let requestedMode = 'connected';
  async function setPreviewMode(mode) {
    // Compare with the last *requested* mode, not state.mode: during rapid clicks the
    // previous load may not have finished yet.
    if (mode === requestedMode && !(mode !== 'unpaired' && state.showPairing)) return;
    requestedMode = mode;
    closeDialog(false);
    state.showPairing = false;
    state.pairing = null;
    state.replyOpen = null;
    await api.setPreviewMode(mode);
    await load();
  }

  function windowControl(action) {
    if (!win) {
      toast(`${action} works in the desktop app. This is the preview.`);
      return undefined;
    }
    if (action === 'Minimize') return win.minimize();
    if (action === 'Maximize') return win.toggleMaximize();
    return win.close();
  }

  // ------------------------------------------------------------------ events

  const onClick = run(async (e) => {
    const preview = e.target.closest('[data-preview]');
    if (preview) return setPreviewMode(preview.dataset.preview);
    const tab = e.target.closest('[data-tab]');
    if (tab) return selectTab(tab.dataset.tab);
    const thread = e.target.closest('[data-thread]');
    if (thread) return openThread(thread.dataset.thread);

    const a = e.target.closest('[data-action]');
    if (!a || a.disabled) return;
    const id = a.closest('[data-id]') ? a.closest('[data-id]').dataset.id : null;

    switch (a.dataset.action) {
      case 'open-settings':
        return openSettings();
      case 'close-dialog':
        return closeDialog();
      case 'win-minimize':
        return windowControl('Minimize');
      case 'win-maximize':
        return windowControl('Maximize');
      case 'win-close':
        return windowControl('Close');
      case 'troubleshoot':
        return openTroubleshoot();
      case 'send-note':
        return openNoteDialog();
      case 'send-file':
        if (!state.device) return toast('Pair your phone first.');
        return el.fileInput.click();
      case 'toggle-clipboard': {
        const on = !state.settings.clipboardSync;
        state.settings.clipboardSync = on;
        setSwitch(a, on);
        await api.updateSetting('clipboardSync', on);
        return toast(on ? 'Clipboard sync on' : 'Clipboard sync off');
      }
      case 'pair-new':
        return startPairing(true);
      case 'cancel-pairing':
        return stopPairing();
      case 'new-code':
        await startPairing(state.pairing ? state.pairing.cancellable : !!state.device);
        return el.pairing.querySelector('button') && el.pairing.querySelector('button').focus();
      case 'simulate-pair':
        a.disabled = true;
        return api.simulatePairing(); // fires the same "paired" event a real scan does
      case 'start-daemon':
        a.disabled = true;
        a.textContent = 'Starting…';
        try {
          await api.startDaemon();
          await load();
        } catch (err) {
          a.disabled = false;
          a.textContent = 'Start the service';
          throw err;
        }
        return undefined;
      case 'clear-all': {
        await api.clearNotifications();
        state.notifications = [];
        state.replyOpen = null;
        renderNotifications();
        renderTabs();
        return el.tabs.notifications.focus();
      }
      case 'dismiss':
        return dismiss(id);
      case 'notif-button': {
        const n = state.notifications.find((x) => x.id === id);
        const index = Number(a.dataset.index);
        if (!n || !api.pressAction) return undefined;
        a.disabled = true;
        try {
          const res = await api.pressAction(id, index);
          toast(res.queued ? `“${n.actions[index]}” will be sent when ${deviceName()} reconnects` : `“${n.actions[index]}” sent to ${deviceName()}`);
        } finally {
          a.disabled = false;
        }
        return undefined;
      }
      case 'reply-open':
        state.replyOpen = id;
        return renderNotifications();
      case 'reply-cancel': {
        state.replyOpen = null;
        renderNotifications();
        const btn = $(`.notif[data-id="${CSS.escape(id)}"] [data-action="reply-open"]`, el.panels.notifications);
        return btn && btn.focus();
      }
      case 'cancel-transfer': {
        const t = state.transfers.find((x) => x.id === id);
        await api.cancelTransfer(id);
        state.transfers = state.transfers.filter((x) => x.id !== id);
        renderFiles();
        renderTabs();
        el.tabs.files.focus();
        return t && toast(`Cancelled “${t.name}”`);
      }
      case 'show-in-folder':
        return toast('Opens ~/Downloads/PhoneLink in the desktop app.');
      case 'copy-id': {
        const text = state.computer ? state.computer.id : '';
        try {
          await navigator.clipboard.writeText(text);
          return toast('Id copied');
        } catch {
          return toast(`Copy failed. The id is ${text}`);
        }
      }
      case 'unpair-ask': {
        const slot = a.closest('[data-unpair-slot]');
        slot.innerHTML = `<span class="confirm-row"><button type="button" class="btn btn-sm" data-action="unpair-cancel">Keep</button>
          <button type="button" class="btn btn-danger btn-sm" data-action="unpair-confirm">Unpair</button></span>`;
        return $('[data-action="unpair-cancel"]', slot).focus();
      }
      case 'unpair-cancel': {
        const slot = a.closest('[data-unpair-slot]');
        slot.innerHTML = '<button type="button" class="btn btn-danger btn-sm" data-action="unpair-ask">Unpair</button>';
        return $('[data-action="unpair-ask"]', slot).focus();
      }
      case 'unpair-confirm': {
        const row = a.closest('[data-device-id]');
        const target = state.devices.find((x) => x.id === row.dataset.deviceId);
        if (!target) return undefined;
        const name = target.name;
        a.disabled = true;
        await api.unpair(target.id);
        closeDialog(false);
        await load();
        return toast(`Unpaired ${name}. It can no longer connect.`);
      }
      case 'toggle-app': {
        const name = a.dataset.name;
        const app = state.settings.apps.find((x) => x.name === name);
        if (!app) return;
        app.enabled = !app.enabled;
        setSwitch(a, app.enabled);
        await api.setAppMirrored(name, app.enabled);
        applyNotifications(await fetchNotifications());
        renderNotifications();
        return renderTabs();
      }
      case 'toggle-setting': {
        const key = a.dataset.key;
        const on = !state.settings[key];
        state.settings[key] = on;
        setSwitch(a, on);
        try {
          await api.updateSetting(key, on);
        } catch (err) {
          state.settings[key] = !on;
          setSwitch(a, !on);
          throw err;
        }
        return undefined;
      }
      default:
        return undefined;
    }
  });

  document.addEventListener('click', onClick);

  // preventDefault must run synchronously: forms submitted with requestSubmit() (Enter,
  // Ctrl+Enter) would otherwise navigate the page before an async handler gets to it.
  const submitHandlers = { reply: run(submitReply), note: run(submitNote) };
  const sendSmsSafe = run(sendSms);
  document.addEventListener('submit', (e) => {
    const form = e.target;
    e.preventDefault(); // nothing in this app is a real HTML form submission
    if (form === el.composer) sendSmsSafe();
    else if (submitHandlers[form.dataset.form]) submitHandlers[form.dataset.form](form);
  });

  document.addEventListener('input', (e) => {
    const t = e.target;
    if (t === el.composerInput) updateComposer();
    else if (t === el.threadSearch) {
      state.threadSearch = t.value;
      renderThreads();
    } else if (t.name === 'reply') {
      const article = t.closest('.notif');
      if (article) state.replyDrafts.set(article.dataset.id, t.value);
    }
  });

  el.composerInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      if (!el.composerSend.disabled) el.composer.requestSubmit();
    }
  });

  // Arrow-key navigation between tabs (WAI-ARIA tabs pattern).
  $('.tabs').addEventListener('keydown', (e) => {
    const names = Object.keys(el.tabs);
    const i = names.indexOf(state.tab);
    let next = null;
    if (e.key === 'ArrowRight') next = names[(i + 1) % names.length];
    else if (e.key === 'ArrowLeft') next = names[(i - 1 + names.length) % names.length];
    else if (e.key === 'Home') next = names[0];
    else if (e.key === 'End') next = names[names.length - 1];
    if (next) {
      e.preventDefault();
      selectTab(next, true);
    }
  });

  document.addEventListener('keydown', (e) => {
    if (dialog) {
      if (e.key === 'Escape') {
        e.preventDefault();
        closeDialog();
        return;
      }
      if (e.key === 'Tab') {
        const f = focusables(dialog.el);
        if (!f.length) {
          e.preventDefault();
          return;
        }
        const first = f[0];
        const last = f[f.length - 1];
        if (!dialog.el.contains(document.activeElement)) {
          e.preventDefault();
          first.focus();
        } else if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
      return;
    }
    if (e.key === 'Escape' && state.replyOpen) {
      const id = state.replyOpen;
      state.replyOpen = null;
      renderNotifications();
      const btn = $(`.notif[data-id="${CSS.escape(id)}"] [data-action="reply-open"]`, el.panels.notifications);
      if (btn) btn.focus();
    }
  });

  el.fileInput.addEventListener(
    'change',
    run(async () => {
      const files = [...el.fileInput.files];
      el.fileInput.value = ''; // allow choosing the same file again
      await sendFiles(files);
    }),
  );

  // Drag and drop files anywhere on the window.
  let dragDepth = 0;
  const hasFiles = (e) => !!e.dataTransfer && [...e.dataTransfer.types].includes('Files');
  el.win.addEventListener('dragenter', (e) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    dragDepth++;
    el.win.classList.add('dragging');
    if (state.device && !state.showPairing && !dialog) selectTab('files');
  });
  el.win.addEventListener('dragover', (e) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = 'copy';
  });
  el.win.addEventListener('dragleave', (e) => {
    if (!hasFiles(e)) return;
    dragDepth = Math.max(0, dragDepth - 1);
    if (!dragDepth) el.win.classList.remove('dragging');
  });
  const sendFilesSafe = run(sendFiles);
  el.win.addEventListener('drop', (e) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    dragDepth = 0;
    el.win.classList.remove('dragging');
    sendFilesSafe([...e.dataTransfer.files]);
  });
  // Never let a dropped file replace the whole page.
  document.addEventListener('dragover', (e) => e.preventDefault());
  document.addEventListener('drop', (e) => e.preventDefault());

  // ------------------------------------------------------------------ timers

  setInterval(() => {
    if (state.showPairing && state.pairing && !state.pairing.loading && !state.pairing.error) {
      const left = Math.max(0, Math.ceil((state.pairing.expiresAt - Date.now()) / 1000));
      const cd = $('[data-countdown]', el.pairing);
      if (left === 0 && !$('.qr-box.expired', el.pairing)) {
        const hadFocus = el.pairing.contains(document.activeElement);
        renderPairing();
        if (hadFocus) $('[data-action="new-code"]', el.pairing).focus();
      } else if (cd && left > 0) {
        cd.textContent = `Expires in ${Math.floor(left / 60)}:${pad(left % 60)} · single use`;
      }
    }
  }, 1000);

  setInterval(() => {
    const c = $('[data-clock]', el.device);
    if (c) c.textContent = clock(Date.now());
    for (const t of document.querySelectorAll('[data-ts]')) t.textContent = relTime(Number(t.dataset.ts));
  }, 30 * 1000);

  setInterval(tickTransfers, 500);

  // ------------------------------------------------------------------ start

  for (const [action, name] of [['open-settings', 'settings'], ['win-minimize', 'minimize'], ['win-maximize', 'maximize'], ['win-close', 'close']]) {
    $(`.titlebar [data-action="${action}"]`).innerHTML = icon(name);
  }
  if (win) {
    document.body.classList.add('in-app');
    const titlebar = $('.titlebar');
    titlebar.addEventListener('mousedown', (e) => {
      if (e.button !== 0 || e.target.closest('button')) return;
      if (e.detail === 2) win.toggleMaximize();
      else win.startDragging();
    });
    for (const h of document.querySelectorAll('[data-resize]')) {
      h.addEventListener('mousedown', (e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        win.startResize(h.dataset.resize);
      });
    }
    const syncMaximized = () =>
      win.isMaximized().then((m) => document.body.classList.toggle('maximized', m)).catch(() => {});
    win.onResized(syncMaximized);
    syncMaximized();
  }
  run(load)();
})();
