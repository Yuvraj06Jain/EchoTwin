// Real backend for the desktop app: Tauri commands that talk to phonelinkd.
//
// Loaded after mock-api.js. Inside the app it replaces the sample-data API; in a normal
// browser (the preview) it does nothing. `phonelink-app --demo` keeps the sample data but
// still uses the real window controls.
'use strict';

(function () {
  const T = window.__TAURI__;
  if (!T) return;

  const win = T.window.getCurrentWindow();
  window.PhoneLinkWindow = {
    minimize: () => win.minimize(),
    toggleMaximize: () => win.toggleMaximize(),
    close: () => win.close(),
    startDragging: () => win.startDragging(),
    startResize: (direction) => win.startResizeDragging(direction),
    isMaximized: () => win.isMaximized(),
    onResized: (cb) => win.onResized(cb),
  };

  if (window.__PHONELINK_DEMO__) return;

  async function invoke(cmd, args) {
    try {
      return await T.core.invoke(cmd, args);
    } catch (e) {
      const msg = typeof e === 'string' ? e : (e && e.message) || String(e);
      const err = new Error(msg === 'daemon-down' ? 'The PhoneLink service is not running' : msg);
      if (msg === 'daemon-down') err.code = 'daemon-down';
      throw err;
    }
  }

  const listeners = { event: [], daemon: [], pairing: [], open: [], drop: [] };
  const SMS_STATUS = { sent: 'sent', sending: 'sending', failed: 'failed' };
  const toThread = (t) => ({
    id: t.id,
    name: t.name || t.address,
    number: t.address,
    unread: t.unread,
    hasOlder: t.has_older,
    messages: t.messages.map((m) => ({
      id: m.id,
      from: m.from_me ? 'me' : 'them',
      text: m.body,
      time: m.date_ms,
      status: SMS_STATUS[m.state],
      error: m.error || '',
    })),
  });
  const toTransfer = (t) => ({
    id: t.id,
    name: t.name,
    size: t.size,
    done: t.done,
    direction: t.direction === 'outgoing' ? 'to-phone' : 'from-phone',
    state: t.state,
    message: t.message || '',
    path: t.path || null,
    deviceName: t.device_name,
  });
  let notes = [];
  let noteSeq = 0;
  let notificationsOn = true;
  let syncDismissals = false;
  let clipboardSync = false;
  let allowLock = false;
  let knownApps = []; // [{ package, app, muted }] from the service

  T.event.listen('phonelink://drop', ({ payload }) => listeners.drop.forEach((cb) => cb(payload)));
  T.event.listen('phonelink://event', ({ payload }) => {
    if (payload.kind === 'transfer_changed' && payload.item) payload = { ...payload, item: toTransfer(payload.item) };
    if (payload.kind === 'text') {
      notes.unshift({
        id: `note-${++noteSeq}`,
        app: 'PhoneLink',
        title: payload.name,
        text: payload.text,
        time: Date.now(),
        canReply: false,
      });
    }
    listeners.event.forEach((cb) => cb(payload));
  });
  T.event.listen('phonelink://daemon', ({ payload }) => listeners.daemon.forEach((cb) => cb(payload)));
  T.event.listen('phonelink://pairing', ({ payload }) => listeners.pairing.forEach((cb) => cb(payload)));
  // The window was asked to come forward (a popup was clicked while it was open).
  T.event.listen('phonelink://open', ({ payload }) => listeners.open.forEach((cb) => cb(payload)));

  const toDevice = (d) => ({
    id: d.id,
    name: d.name,
    model: null, // not reported by the phone yet
    connected: d.connected,
    pending: d.pending,
    network: null,
    battery: null,
    lastSeen: null,
  });

  window.PhoneLinkAPI = {
    // What the phone and service can do today. The UI hides or explains the rest.
    capabilities: {
      preview: false,
      notificationMirroring: true,
      syncDismissals: true,
      notificationButtons: true,
      sms: true,
      clipboard: true,
      files: true,
      lockFromPhone: true,
      media: true,
      deviceDetails: false,
      appFilters: true,
      discoveryToggle: false,
    },

    async status() {
      const s = await invoke('status');
      notificationsOn = s.notifications;
      syncDismissals = !!s.sync_dismissals;
      clipboardSync = !!s.clipboard_sync;
      allowLock = !!s.allow_lock;
      const devices = s.devices.map(toDevice);
      const device = devices.find((d) => d.connected) || devices[0] || null;
      return {
        computer: s.computer,
        devices,
        device,
        mode: device ? (device.connected ? 'connected' : 'offline') : 'unpaired',
      };
    },

    // Mirrored notifications from the phone, plus notes typed in the phone app.
    async notifications() {
      const r = await invoke('notifications');
      knownApps = r.apps;
      const mirrored = r.items.map((n) => ({
        id: n.id,
        app: n.app,
        package: n.package,
        title: n.title,
        text: n.text,
        time: n.posted_ms,
        canReply: n.can_reply,
        actions: n.actions,
        removedOnPhone: n.removed_on_phone,
      }));
      return [...notes.map((n) => ({ ...n })), ...mirrored];
    },
    async dismiss(id) {
      if (id.startsWith('note-')) {
        notes = notes.filter((n) => n.id !== id);
        return;
      }
      await invoke('dismiss_notification', { id });
    },
    async clearNotifications() {
      notes = [];
      await invoke('clear_notifications');
    },
    async reply(id, text) {
      const deliveredNow = await invoke('reply_notification', { id, text });
      return { queued: !deliveredNow };
    },
    async pressAction(id, index) {
      const deliveredNow = await invoke('notification_action', { id, index });
      return { queued: !deliveredNow };
    },
    async appIcons() {
      return invoke('app_icons');
    },

    async threads() {
      return (await invoke('sms_threads')).map(toThread);
    },
    async markRead(threadId) {
      await invoke('sms_mark_read', { thread: threadId });
    },
    // The phone sends it; the result arrives later as an event (sms_send_result).
    async sendSms(threadId, text) {
      const id = await invoke('send_sms', { thread: threadId, text });
      return { id, from: 'me', text, time: Date.now(), status: 'sending' };
    },
    async loadOlder(threadId) {
      await invoke('sms_load_older', { thread: threadId });
    },

    async transfers() {
      return (await invoke('transfers')).map(toTransfer);
    },
    async sendPaths(paths) {
      return invoke('send_files', { paths });
    },
    // The system file picker; returns the chosen paths (none if cancelled).
    async chooseFiles() {
      return invoke('pick_files');
    },
    async cancelTransfer(id) {
      await invoke('cancel_transfer', { id });
    },
    async clearTransfers() {
      await invoke('clear_transfers');
    },
    async openTransfer(id, reveal) {
      await invoke('open_transfer', { id, reveal });
    },
    onDrop(cb) {
      listeners.drop.push(cb);
    },

    async settings() {
      return {
        desktopNotifications: notificationsOn,
        syncDismissals,
        discovery: true,
        clipboardSync,
        allowLock,
        apps: knownApps.map((a) => ({ name: a.app, package: a.package, enabled: !a.muted })),
      };
    },
    async updateSetting(key, value) {
      if (key === 'desktopNotifications') {
        await invoke('set_notifications', { enabled: value });
        notificationsOn = value;
      } else if (key === 'syncDismissals') {
        await invoke('set_sync_dismissals', { enabled: value });
        syncDismissals = value;
      } else if (key === 'clipboardSync') {
        await invoke('set_clipboard_sync', { enabled: value });
        clipboardSync = value;
      } else if (key === 'allowLock') {
        await invoke('set_allow_lock', { enabled: value });
        allowLock = value;
      }
    },
    async setAppMirrored(name, enabled) {
      const app = knownApps.find((a) => a.app === name);
      if (!app) throw new Error(`unknown app ${name}`);
      await invoke('set_app_muted', { package: app.package, muted: !enabled });
      app.muted = !enabled;
    },

    async sendText(text, deviceId) {
      const deliveredNow = await invoke('send_note', { text, device: deviceId || null });
      return { queued: !deliveredNow };
    },
    async unpair(id) {
      await invoke('unpair', { device: id });
    },

    async startPairing() {
      const p = await invoke('start_pairing');
      return { qr: p.qr, id: p.id, expiresIn: p.expires_in };
    },
    async cancelPairing() {
      await invoke('cancel_pairing');
    },
    async startDaemon() {
      await invoke('start_daemon');
    },
    async reconnect() {
      await invoke('reconnect');
    },

    onEvent(cb) {
      listeners.event.push(cb);
    },

    // What's playing on the phone, and its buttons.
    async media() {
      return invoke('media');
    },
    async mediaControl(action, value) {
      const a = action === 'set_volume' ? { set_volume: value } : action;
      await invoke('media_control', { action: a });
    },
    onDaemon(cb) {
      listeners.daemon.push(cb);
    },
    onPairing(cb) {
      listeners.pairing.push(cb);
    },
    onOpen(cb) {
      listeners.open.push(cb);
    },
  };
})();
