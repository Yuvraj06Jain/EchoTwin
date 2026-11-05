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

  const listeners = { event: [], daemon: [], pairing: [] };
  let notes = [];
  let noteSeq = 0;
  let notificationsOn = true;

  T.event.listen('phonelink://event', ({ payload }) => {
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

  const notYet = (what) => async () => {
    throw new Error(`${what} is not available yet`);
  };

  window.PhoneLinkAPI = {
    // What the phone and service can do today. The UI hides or explains the rest.
    capabilities: {
      preview: false,
      notificationMirroring: false,
      sms: false,
      files: false,
      clipboard: false,
      deviceDetails: false,
      appFilters: false,
      discoveryToggle: false,
    },

    async status() {
      const s = await invoke('status');
      notificationsOn = s.notifications;
      const devices = s.devices.map(toDevice);
      const device = devices.find((d) => d.connected) || devices[0] || null;
      return {
        computer: s.computer,
        devices,
        device,
        mode: device ? (device.connected ? 'connected' : 'offline') : 'unpaired',
      };
    },

    async notifications() {
      return notes.map((n) => ({ ...n }));
    },
    async dismiss(id) {
      notes = notes.filter((n) => n.id !== id);
    },
    async clearNotifications() {
      notes = [];
    },
    reply: notYet('Replying'),

    async threads() {
      return [];
    },
    async markRead() {},
    sendSms: notYet('SMS'),

    async transfers() {
      return [];
    },
    sendFiles: notYet('File transfer'),
    async cancelTransfer() {},
    async transferProgress() {},

    async settings() {
      return { desktopNotifications: notificationsOn, discovery: true, clipboardSync: false, apps: [] };
    },
    async updateSetting(key, value) {
      if (key === 'desktopNotifications') {
        await invoke('set_notifications', { enabled: value });
        notificationsOn = value;
      }
    },
    async setAppMirrored() {},

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

    onEvent(cb) {
      listeners.event.push(cb);
    },
    onDaemon(cb) {
      listeners.daemon.push(cb);
    },
    onPairing(cb) {
      listeners.pairing.push(cb);
    },
  };
})();
