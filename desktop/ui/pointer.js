// The presentation pointer (overlay window "pointer", the whole screen, click-through).
// Positions come from the phone's touchpad as 0..10000 across the screen.
(() => {
  const T = window.__TAURI__;
  const dot = document.getElementById('p');
  const shade = document.getElementById('shade');
  const SIZES = [14, 22, 34]; // dot diameter, px: small, medium, large
  const SPOT = [60, 100, 150]; // spotlight radius, px
  const HIDE_AFTER_MS = 3000;
  let want = null; // the latest pointer event
  let lastMs = 0;
  let frame = 0;

  const hex = (c) => '#' + (c & 0xffffff).toString(16).padStart(6, '0');

  function draw() {
    frame = 0;
    const p = want;
    const on = p && p.visible && Date.now() - lastMs < HIDE_AFTER_MS;
    if (!on) {
      dot.hidden = true;
      shade.hidden = true;
      return;
    }
    const x = (Math.min(10000, Math.max(0, p.x)) / 10000) * window.innerWidth;
    const y = (Math.min(10000, Math.max(0, p.y)) / 10000) * window.innerHeight;
    const size = SIZES[Math.min(2, Math.max(0, p.size | 0))];
    const color = hex(p.color);
    if (p.shape === 'spotlight') {
      dot.hidden = true;
      shade.hidden = false;
      const r = SPOT[Math.min(2, Math.max(0, p.size | 0))];
      shade.style.background = `radial-gradient(circle ${r}px at ${x}px ${y}px, transparent 0, transparent ${r - 1}px, rgba(0,0,0,.7) ${r}px)`;
      return;
    }
    shade.hidden = true;
    dot.hidden = false;
    const d = p.shape === 'ring' ? Math.round(size * 1.6) : size;
    dot.style.width = dot.style.height = `${d}px`;
    if (p.shape === 'ring') {
      dot.style.background = 'transparent';
      dot.style.border = `3px solid ${color}`;
      dot.style.boxShadow = `0 0 8px ${color}`;
    } else {
      dot.style.background = color;
      dot.style.border = '0';
      dot.style.boxShadow = `0 0 ${Math.round(d * 0.8)}px ${color}`;
    }
    const half = d / 2 + (p.shape === 'ring' ? 3 : 0);
    dot.style.transform = `translate(${x - half}px, ${y - half}px)`;
  }

  function show(p) {
    want = p;
    lastMs = Date.now();
    if (!frame) frame = requestAnimationFrame(draw);
  }

  T.event.listen('phonelink://event', ({ payload }) => {
    if (payload.kind === 'pointer') show(payload);
  });
  // Nothing for a while (the phone went away): hide.
  setInterval(() => {
    if (want && want.visible && Date.now() - lastMs >= HIDE_AFTER_MS && !frame) frame = requestAnimationFrame(draw);
  }, 500);
})();
