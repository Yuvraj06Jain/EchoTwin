// Sample-data backend for the UI preview.
//
// It has the same shape as the real backend the desktop app will use (Tauri commands
// talking to phonelinkd), so app.js does not change when the real one is plugged in.
// Every method is async, like the real thing, with a little artificial latency.
'use strict';

(function () {
  const MIN = 60 * 1000;
  const now = Date.now();
  const ago = (minutes) => now - minutes * MIN;
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const clone = (v) => JSON.parse(JSON.stringify(v));
  let seq = 1000;
  const nextId = (prefix) => `${prefix}-${++seq}`;

  const computer = { name: 'kubuntu-laptop', id: '405F-AEB0-D952-DF66' };

  const phone = {
    id: '9C21-7E04-B3A8-51DF',
    name: 'Galaxy S24 FE',
    model: 'Samsung · Android 15',
    network: 'Home-5G',
    battery: 76,
    charging: false,
  };

  let mode = 'connected'; // connected | offline | unpaired
  let paired = true;
  let lastSeen = ago(4);

  const db = {
    notifications: [
      { id: 'n1', app: 'WhatsApp', title: 'Rahul', text: 'Are you coming to the lab today? Sir moved the viva to 3 pm.', time: ago(2), canReply: true, actions: ['Mark as read'] },
      { id: 'n2', app: 'WhatsApp', title: 'CSE 3rd year', text: 'Ananya: Notes for unit 4 are in the drive folder', time: ago(9), canReply: true },
      { id: 'n3', app: 'Gmail', title: 'Placement Cell', text: 'Registration for the November drive closes tomorrow at 5 pm.', time: ago(26), canReply: false },
      { id: 'n4', app: 'Calendar', title: 'Operating Systems lab', text: 'Today, 2:00 – 5:00 pm · Lab complex, room 214', time: ago(48), canReply: false },
      { id: 'n5', app: 'Messages', title: 'Mom', text: 'Call me when you are free', time: ago(75), canReply: true },
      { id: 'n6', app: 'Swiggy', title: 'Order delivered', text: 'Your order from Campus Cafe has been delivered.', time: ago(190), canReply: false },
    ],
    threads: [
      {
        id: 't1', name: 'Mom', number: '+91 98xxxx4410', unread: 1,
        messages: [
          { id: 'm1', from: 'them', text: 'Did you reach the hostel?', time: ago(60 * 26) },
          { id: 'm2', from: 'me', text: 'Yes, reached at 9. Train was late.', time: ago(60 * 26 - 3), status: 'delivered' },
          { id: 'm3', from: 'them', text: 'Ok good. Eat something.', time: ago(60 * 26 - 5) },
          { id: 'm4', from: 'them', text: 'Call me when you are free', time: ago(75) },
        ],
      },
      {
        id: 't2', name: 'Rahul', number: '+91 97xxxx2231', unread: 0,
        messages: [
          { id: 'm5', from: 'them', text: 'Bro send the OS assignment pdf', time: ago(60 * 5) },
          { id: 'm6', from: 'me', text: 'Sending in 5 min', time: ago(60 * 5 - 2), status: 'delivered' },
          { id: 'm7', from: 'me', text: 'Sent it on the group', time: ago(60 * 4), status: 'delivered' },
          { id: 'm8', from: 'them', text: 'Got it, thanks!', time: ago(60 * 4 - 1) },
        ],
      },
      {
        id: 't3', name: 'AX-HDFCBK', number: 'AX-HDFCBK', unread: 0,
        messages: [
          { id: 'm9', from: 'them', text: 'Rs. 240.00 debited from a/c XX4521 on 05-11 at CAMPUS CAFE. Not you? Call 1800 xxx xxxx.', time: ago(60 * 3) },
        ],
      },
      {
        id: 't4', name: 'Dad', number: '+91 98xxxx0087', unread: 0,
        messages: [
          { id: 'm10', from: 'them', text: 'Fees paid for this semester. Check the portal.', time: ago(60 * 24 * 3) },
          { id: 'm11', from: 'me', text: 'Got the receipt. Thank you!', time: ago(60 * 24 * 3 - 10), status: 'delivered' },
        ],
      },
    ],
    transfers: [
      { id: 'f1', name: 'Lecture-09-recording.mp4', size: 812 * 1024 * 1024, direction: 'to-phone', progress: 0.46, state: 'active' },
      { id: 'f2', name: 'IMG_20261105_0941.jpg', size: 3.1 * 1024 * 1024, direction: 'from-phone', progress: 1, state: 'done' },
      { id: 'f3', name: 'OS-lab-report-final.pdf', size: 2.4 * 1024 * 1024, direction: 'to-phone', progress: 1, state: 'done' },
    ],
    settings: {
      desktopNotifications: true,
      syncDismissals: false,
      discovery: true,
      clipboardSync: true,
      allowLock: true,
      apps: [
        { name: 'WhatsApp', enabled: true },
        { name: 'Messages', enabled: true },
        { name: 'Gmail', enabled: true },
        { name: 'Calendar', enabled: true },
        { name: 'Swiggy', enabled: true },
        { name: 'Instagram', enabled: false },
        { name: 'Bank apps', enabled: false },
      ],
    },
  };

  function deviceView() {
    if (!paired) return null;
    return { ...phone, connected: mode === 'connected', lastSeen: mode === 'connected' ? Date.now() : lastSeen };
  }

  const pairingListeners = [];

  const api = {
    // The preview shows every planned feature.
    capabilities: {
      preview: true,
      notificationMirroring: true,
      sms: true,
      files: true,
      clipboard: true,
      lockFromPhone: true,
      deviceDetails: true,
      appFilters: true,
      discoveryToggle: true,
      syncDismissals: true,
      notificationButtons: true,
    },

    // ---- preview-only controls (not part of the real backend)
    async setPreviewMode(next) {
      mode = next;
      paired = next !== 'unpaired';
      if (next === 'offline') lastSeen = ago(4);
      for (const t of db.transfers) {
        if (t.state === 'active' && next !== 'connected') t.state = 'paused';
        else if (t.state === 'paused' && next === 'connected') t.state = 'active';
      }
    },
    async simulatePairing() {
      await wait(400);
      mode = 'connected';
      paired = true;
      const d = deviceView();
      pairingListeners.forEach((cb) => cb({ kind: 'paired', name: d.name }));
      return d;
    },

    // ---- the real API surface
    async status() {
      await wait(40);
      const device = deviceView();
      return { computer: clone(computer), mode, device: clone(device), devices: device ? [clone(device)] : [] };
    },
    async startPairing() {
      await wait(120);
      return { qr: window.PREVIEW_QR, id: computer.id, expiresIn: 120 };
    },
    async cancelPairing() {},
    async startDaemon() {},
    async reconnect() {
      await wait(100);
    },
    onEvent() {},
    onDaemon() {},
    onPairing(cb) {
      pairingListeners.push(cb);
    },
    async notifications() {
      await wait(60);
      const enabled = new Set(db.settings.apps.filter((a) => a.enabled).map((a) => a.name));
      return clone(db.notifications.filter((n) => enabled.has(n.app)));
    },
    async dismiss(id) {
      await wait(80);
      db.notifications = db.notifications.filter((n) => n.id !== id);
    },
    async clearNotifications() {
      await wait(80);
      db.notifications = [];
    },
    async reply(id, text) {
      await wait(250);
      if (!text.trim()) throw new Error('empty reply');
      return { queued: mode !== 'connected' };
    },
    async pressAction(id, index) {
      await wait(150);
      const n = db.notifications.find((x) => x.id === id);
      if (!n || !n.actions || !n.actions[index]) throw new Error('That button is no longer available');
      return { queued: mode !== 'connected' };
    },
    async appIcons() {
      return {};
    },
    async threads() {
      await wait(60);
      return clone(db.threads);
    },
    async markRead(threadId) {
      const t = db.threads.find((x) => x.id === threadId);
      if (t) t.unread = 0;
    },
    async sendSms(threadId, text) {
      const t = db.threads.find((x) => x.id === threadId);
      if (!t) throw new Error('unknown conversation');
      const msg = { id: nextId('m'), from: 'me', text, time: Date.now(), status: mode === 'connected' ? 'delivered' : 'queued' };
      await wait(mode === 'connected' ? 700 : 150);
      t.messages.push(msg);
      return clone(msg);
    },
    async sendText(text /* , deviceId */) {
      await wait(150);
      if (!text.trim()) throw new Error('empty text');
      return { queued: mode !== 'connected' };
    },
    async transfers() {
      await wait(40);
      return clone(db.transfers);
    },
    async sendFiles(files) {
      await wait(80);
      const added = files.map((f) => ({
        id: nextId('f'),
        name: f.name,
        size: f.size,
        direction: 'to-phone',
        progress: 0,
        state: mode === 'connected' ? 'active' : 'paused',
      }));
      db.transfers.unshift(...added);
      return clone(added);
    },
    async cancelTransfer(id) {
      await wait(60);
      db.transfers = db.transfers.filter((t) => t.id !== id);
    },
    async transferProgress(id, progress, state) {
      const t = db.transfers.find((x) => x.id === id);
      if (t) {
        t.progress = progress;
        t.state = state;
      }
    },
    async settings() {
      await wait(40);
      return clone(db.settings);
    },
    async updateSetting(key, value) {
      await wait(60);
      db.settings[key] = value;
    },
    async setAppMirrored(name, enabled) {
      await wait(60);
      const app = db.settings.apps.find((a) => a.name === name);
      if (app) app.enabled = enabled;
    },
    async unpair(id) {
      await wait(200);
      if (id === phone.id) {
        paired = false;
        mode = 'unpaired';
      }
    },
  };

  window.PhoneLinkAPI = api;
})();
