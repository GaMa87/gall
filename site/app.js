/* Gall website progressive enhancement:
   - Tabbed screenshot & GIF showcases (ARIA tablist + arrow keys)
   - OS install switcher + copy-to-clipboard feedback
   - Dark / Light theme switch (respects saved preference & system theme)
   - Gall-inspired `?` keyboard quick-jump / shortcut palette
   - Reduced-motion-safe scroll reveals */

(function () {
  "use strict";

  const root = document.documentElement;
  root.classList.add("js-ready");

  /* ------------------------------------------------------------------------
     1. Theme toggle (Dark default to match Gall, switchable to Light)
     ------------------------------------------------------------------------ */
  const THEME_KEY = "gall-site-theme";
  const themeBtn = document.getElementById("theme-toggle");
  const themeLabel = document.getElementById("theme-toggle-label");

  function applyTheme(theme) {
    const isLight = theme === "light";
    if (isLight) {
      root.setAttribute("data-theme", "light");
    } else {
      root.removeAttribute("data-theme");
    }
    if (themeBtn) {
      themeBtn.setAttribute("aria-pressed", String(isLight));
      themeBtn.setAttribute("title", isLight ? "Switch to dark theme" : "Switch to light theme");
    }
    if (themeLabel) {
      themeLabel.textContent = isLight ? "Dark" : "Light";
    }
  }

  let savedTheme = null;
  try {
    savedTheme = localStorage.getItem(THEME_KEY);
  } catch (_) {}
  if (savedTheme === "light" || savedTheme === "dark") {
    applyTheme(savedTheme);
  } else {
    applyTheme("dark");
  }

  function toggleTheme() {
    const next = root.getAttribute("data-theme") === "light" ? "dark" : "light";
    applyTheme(next);
    try {
      localStorage.setItem(THEME_KEY, next);
    } catch (_) {}
  }

  if (themeBtn) {
    themeBtn.addEventListener("click", toggleTheme);
  }

  /* ------------------------------------------------------------------------
     2. Reusable ARIA Tablists (Hero layouts, Motion reel, Compare lab, Install)
     ------------------------------------------------------------------------ */
  function initTabGroup(groupEl) {
    const tabs = Array.from(groupEl.querySelectorAll('[role="tab"]'));
    if (!tabs.length) return;

    const captionEl = groupEl.querySelector("[data-stage-caption]");
    const hintEl = groupEl.querySelector("[data-stage-hint]");
    const pathEl = groupEl.querySelector("[data-stage-path]");

    function activate(tab, focusTab) {
      const targetId = tab.getAttribute("aria-controls");
      tabs.forEach((t) => {
        const active = t === tab;
        t.setAttribute("aria-selected", String(active));
        t.tabIndex = active ? 0 : -1;
        const panelId = t.getAttribute("aria-controls");
        const panel = panelId ? document.getElementById(panelId) : null;
        if (!panel) return;
        if (panel.classList.contains("stage-panel")) {
          panel.classList.toggle("is-active", active);
          panel.setAttribute("aria-hidden", String(!active));
          if (active) {
            const img = panel.querySelector("img[data-src]");
            if (img) {
              img.src = img.getAttribute("data-src");
              img.removeAttribute("data-src");
            }
          }
        } else {
          panel.hidden = !active;
        }
      });

      if (captionEl && tab.dataset.caption) {
        captionEl.innerHTML = tab.dataset.caption;
      }
      if (hintEl && tab.dataset.hint) {
        hintEl.innerHTML = tab.dataset.hint;
      }
      if (pathEl && tab.dataset.path) {
        pathEl.textContent = tab.dataset.path;
      }
      if (focusTab) {
        tab.focus();
      }
      if (targetId && groupEl.dataset.syncTheme === "true") {
        // Optional subtle hint only; never overrides user theme involuntarily
      }
    }

    tabs.forEach((tab, index) => {
      tab.addEventListener("click", () => activate(tab, false));
      tab.addEventListener("keydown", (event) => {
        let nextIndex = null;
        if (event.key === "ArrowRight" || event.key === "ArrowDown") {
          nextIndex = (index + 1) % tabs.length;
        } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
          nextIndex = (index - 1 + tabs.length) % tabs.length;
        } else if (event.key === "Home") {
          nextIndex = 0;
        } else if (event.key === "End") {
          nextIndex = tabs.length - 1;
        }
        if (nextIndex !== null) {
          event.preventDefault();
          activate(tabs[nextIndex], true);
        }
      });
    });
  }

  document.querySelectorAll("[data-tabs]").forEach(initTabGroup);

  /* Auto-select user's likely OS in the Install switcher */
  const installGroup = document.querySelector('[data-tabs="install"]');
  if (installGroup) {
    const ua = (navigator.userAgent || "").toLowerCase();
    let osKey = "fedora";
    if (ua.includes("win")) osKey = "windows";
    else if (ua.includes("mac")) osKey = "macos";
    else if (ua.includes("linux") && !ua.includes("fedora")) osKey = "linux";
    const osTab = installGroup.querySelector(`[data-os="${osKey}"]`);
    if (osTab) osTab.click();
  }

  /* ------------------------------------------------------------------------
     3. Copy-to-clipboard for install commands
     ------------------------------------------------------------------------ */
  document.querySelectorAll("[data-copy]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const text = btn.getAttribute("data-copy");
      if (!text) return;
      const original = btn.textContent;
      try {
        await navigator.clipboard.writeText(text);
        btn.textContent = "Copied";
        btn.classList.add("is-copied");
      } catch (_) {
        const range = document.createRange();
        const code = btn.parentElement && btn.parentElement.querySelector("code");
        if (code) {
          range.selectNodeContents(code);
          const sel = window.getSelection();
          if (sel) {
            sel.removeAllRanges();
            sel.addRange(range);
          }
        }
        btn.textContent = "Selected";
      }
      window.setTimeout(() => {
        btn.textContent = original;
        btn.classList.remove("is-copied");
      }, 1800);
    });
  });

  /* ------------------------------------------------------------------------
     4. Signature Interaction: `?` Keyboard Quick-Jump & Shortcut Sheet
     ------------------------------------------------------------------------ */
  const palette = document.getElementById("quick-palette");
  const openPaletteBtn = document.getElementById("palette-open");
  const closePaletteBtn = document.getElementById("palette-close");
  let lastFocus = null;

  function openPalette() {
    if (!palette || !palette.hidden) return;
    lastFocus = document.activeElement;
    palette.hidden = false;
    document.body.style.overflow = "hidden";
    const first = palette.querySelector("a, button");
    if (first) first.focus();
  }

  function closePalette() {
    if (!palette || palette.hidden) return;
    palette.hidden = true;
    document.body.style.overflow = "";
    if (lastFocus && typeof lastFocus.focus === "function") {
      lastFocus.focus();
    }
  }

  if (openPaletteBtn) {
    openPaletteBtn.addEventListener("click", openPalette);
  }
  if (closePaletteBtn) {
    closePaletteBtn.addEventListener("click", closePalette);
  }
  if (palette) {
    palette.addEventListener("click", (event) => {
      if (event.target === palette) closePalette();
    });
    palette.querySelectorAll("[data-palette-jump]").forEach((el) => {
      el.addEventListener("click", () => closePalette());
    });
    const paletteThemeBtn = palette.querySelector("[data-palette-theme]");
    if (paletteThemeBtn) {
      paletteThemeBtn.addEventListener("click", () => {
        toggleTheme();
        closePalette();
      });
    }
  }

  document.addEventListener("keydown", (event) => {
    const tag = (event.target && event.target.tagName) || "";
    const inEditable =
      tag === "INPUT" ||
      tag === "TEXTAREA" ||
      tag === "SELECT" ||
      (event.target && event.target.isContentEditable);
    if (inEditable) return;

    if (event.key === "Escape" && palette && !palette.hidden) {
      event.preventDefault();
      closePalette();
      return;
    }

    if (event.key === "?" && !event.ctrlKey && !event.metaKey && !event.altKey) {
      event.preventDefault();
      if (palette && palette.hidden) openPalette();
      else closePalette();
    }
  });

  /* ------------------------------------------------------------------------
     5. Subtle Scroll Reveal (respects prefers-reduced-motion)
     ------------------------------------------------------------------------ */
  const revealEls = Array.from(document.querySelectorAll("[data-reveal]"));
  const reducedMotion =
    window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  if (reducedMotion || !("IntersectionObserver" in window)) {
    revealEls.forEach((el) => el.classList.add("is-visible"));
  } else {
    const observer = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (entry.isIntersecting) {
            entry.target.classList.add("is-visible");
            observer.unobserve(entry.target);
          }
        });
      },
      { rootMargin: "0px 0px -8% 0px", threshold: 0.05 }
    );
    revealEls.forEach((el) => observer.observe(el));
  }
})();
