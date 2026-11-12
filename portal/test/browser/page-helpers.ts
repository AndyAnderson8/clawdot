// The script injected into every page under test: window.T (query and interaction helpers), the answer to the
// page's own "Are you sure?" dialog, and a guard against the browser's confirm box.
export const HELPERS = `(() => {
  const settle = () => new Promise((r) => setTimeout(r, 50));
  window.T = {
    q: (s) => document.querySelector(s),
    all: (s) => [...document.querySelectorAll(s)],
    text: (s) => (document.querySelector(s)?.textContent ?? null),
    settle,
    set: async (s, v) => { const e = document.querySelector(s); if (!e) throw new Error('no ' + s); e.value = v; e.dispatchEvent(new Event('input', { bubbles: true })); e.dispatchEvent(new Event('change', { bubbles: true })); await settle(); },
    click: async (s) => { const e = document.querySelector(s); if (!e) throw new Error('no ' + s); e.click(); await settle(); },
    clickText: async (scope, text) => { const e = [...document.querySelectorAll(scope + ' button, ' + scope + ' a, ' + scope + ' summary')].find((x) => x.textContent.trim() === text); if (!e) throw new Error('no "' + text + '" in ' + scope); e.click(); await settle(); },
    submit: async (s) => { const f = document.querySelector(s); f.requestSubmit(); await settle(); },
    wide: () => {
      // A wide stack with extra spacing, so a layout that only fits narrow system fonts (Linux CI) shows itself here.
      const st = document.createElement('style');
      st.id = 'widefont';
      st.textContent = 'html *:not(svg):not(svg *) { font-family: Verdana, "DejaVu Sans", "Arial Black", sans-serif !important; letter-spacing: 0.05em !important; } code, pre, .token { font-family: "Courier New", monospace !important; }';
      if (!document.getElementById('widefont')) document.head.append(st);
    },
    // Things a fixed bar or a clipping box would hide: the focus ring's room, and the last row above the bottom bar.
    hidden: () => {
      const out = [];
      const sel = 'a[href], button, input:not([type=hidden]), select, textarea, summary';
      for (const el of document.querySelectorAll(sel)) {
        const r = el.getBoundingClientRect();
        if (r.width === 0 || el.closest('.sr') || getComputedStyle(el).position === 'fixed') continue;
        for (let p = el.parentElement; p && p !== document.body; p = p.parentElement) {
          if (p.classList.contains('tabs')) break; // a strip meant to scroll sideways
          const cs = getComputedStyle(p);
          if (cs.overflowX === 'visible' && cs.overflowY === 'visible') continue;
          const pr = p.getBoundingClientRect();
          if (r.left - 5 < pr.left - 0.5 || r.right + 5 > pr.right + 0.5) { out.push('focus ring clipped: ' + el.tagName.toLowerCase() + (el.id ? '#' + el.id : '') + ' by ' + p.tagName.toLowerCase() + (p.id ? '#' + p.id : '') + ' (' + Math.round(r.left) + '..' + Math.round(r.right) + ' in ' + Math.round(pr.left) + '..' + Math.round(pr.right) + ')'); break; }
        }
      }
      window.scrollTo(0, document.documentElement.scrollHeight);
      const nav = document.querySelector('#tabs');
      const foot = document.querySelector('#foot');
      if (nav && foot && getComputedStyle(nav).position === 'fixed' && foot.getBoundingClientRect().bottom > nav.getBoundingClientRect().top + 0.5)
        out.push('the bottom bar covers the end of the page (footer bottom ' + Math.round(foot.getBoundingClientRect().bottom) + ' > bar top ' + Math.round(nav.getBoundingClientRect().top) + ')');
      window.scrollTo(0, 0);
      return out.slice(0, 6);
    },
    overflow: () => {
      const vw = document.documentElement.clientWidth;
      const clipped = (el) => { for (let p = el.parentElement; p; p = p.parentElement) { const o = getComputedStyle(p).overflowX; if (o !== 'visible' && p.getBoundingClientRect().right <= vw + 0.5) return true; } return false; };
      const out = [];
      for (const el of document.querySelectorAll('body *')) {
        const r = el.getBoundingClientRect();
        if (r.width === 0 || r.right <= vw + 0.5 || clipped(el)) continue;
        if (getComputedStyle(el).position === 'fixed' && r.right <= vw + 0.5) continue;
        out.push(el.tagName.toLowerCase() + (el.id ? '#' + el.id : '') + (el.className && typeof el.className === 'string' ? '.' + el.className.trim().split(/\\s+/).join('.') : '') + ' right=' + Math.round(r.right) + ' width=' + Math.round(r.width) + ' scrollWidth=' + el.scrollWidth + ' "' + (el.textContent || '').trim().replace(/\\s+/g, ' ').slice(0, 40) + '"');
        if (out.length >= 6) break;
      }
      if (!out.length && document.documentElement.scrollWidth > vw) {
        // Nothing visibly sticks out, so content inside something does: list boxes whose own content is wider than they are.
        const wider = (el) => el.clientWidth > 0 && el.scrollWidth > el.clientWidth + 1 && getComputedStyle(el).overflowX === "visible";
        const inner = [...document.querySelectorAll("body *")].filter((el) => wider(el) && ![...el.children].some(wider)).slice(0, 5);
        for (const el of inner) out.push("(content wider than box) " + el.tagName.toLowerCase() + (el.id ? "#" + el.id : "") + (typeof el.className === "string" && el.className ? "." + el.className.trim().split(/\\s+/).join(".") : "") + " clientWidth=" + el.clientWidth + " scrollWidth=" + el.scrollWidth);
      }
      return { viewport: vw, scrollWidth: document.documentElement.scrollWidth, offenders: out };
    },
    rows: (s) => [...document.querySelectorAll(s ?? '#panel ul.rows > li')].map((li) => li.textContent),
  };
  // "Are you sure?" is the page's own dialog. The suite answers it as a person would, by pressing one of its two
  // buttons: __answer is true, false, or a function of the question's text.
  window.__answer = () => true;
  new MutationObserver(() => {
    const yes = document.getElementById('confirmyes');
    if (!yes || yes.dataset.pressed) return;
    yes.dataset.pressed = '1';
    const a = typeof window.__answer === 'function' ? window.__answer(document.getElementById('confirmtext').textContent) : window.__answer;
    document.getElementById(a ? 'confirmyes' : 'confirmno').click();
  }).observe(document.documentElement, { childList: true, subtree: true });
  window.confirm = () => { throw new Error('the browser confirm box was used'); };
  return true;
})()`;
