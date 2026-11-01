// fframes Studio landing page: product tour tabs, copy button, the header's glass, the page
// timeline, the export race and scroll motion (reveals, the showcase tilting upright, hero cards
// and far clouds drifting, the sky going from dawn to dusk). Motion is skipped under
// prefers-reduced-motion; everything stays visible without this script.
(() => {
  const root = document.documentElement;
  root.classList.add("js-enabled");
  const still = matchMedia("(prefers-reduced-motion: reduce)").matches;

  // Tour tabs: one screenshot at a time, arrow keys move between tabs.
  const tablist = document.querySelector("[data-tabs]");
  if (tablist) {
    const tabs = [...tablist.querySelectorAll('[role="tab"]')];
    const select = next => {
      for (const tab of tabs) {
        const on = tab === next;
        tab.setAttribute("aria-selected", String(on));
        tab.tabIndex = on ? 0 : -1;
        document.getElementById(tab.getAttribute("aria-controls"))?.classList.toggle("is-active", on);
      }
    };
    tabs.forEach((tab, i) => {
      tab.addEventListener("click", () => select(tab));
      tab.addEventListener("keydown", e => {
        const to = { ArrowRight: i + 1, ArrowLeft: i - 1, Home: 0, End: tabs.length - 1 }[e.key];
        if (to === undefined) return;
        e.preventDefault();
        const next = tabs[(to + tabs.length) % tabs.length];
        select(next);
        next.focus();
      });
    });
  }

  // Hero player: four example videos. A clip shows its frame and fills in the window title,
  // timecode, prompt and the command that rendered it. They advance every five seconds until
  // someone picks one, and only when motion is allowed.
  const show = document.querySelector("[data-show]");
  if (show) {
    const clips = [...show.querySelectorAll(".clip")];
    const frames = [...show.querySelectorAll(".player-frame")];
    // The three hidden frames carry data-srcset so they do not compete with the first paint;
    // each one loads while the browser is idle, one rotation ahead of being shown.
    const load = img => {
      if (!img.dataset.srcset) return;
      img.srcset = img.dataset.srcset;
      delete img.dataset.srcset;
    };
    const idle = window.requestIdleCallback ?? (fn => setTimeout(fn, 200));
    const preload = i => idle(() => load(frames[i % frames.length]), { timeout: 3000 });
    const set = (sel, text) => {
      const el = document.querySelector(sel);
      if (el) el.textContent = text;
    };
    let at = 0;
    const pick = i => {
      at = i;
      clips.forEach((clip, k) => {
        clip.classList.toggle("is-on", k === i);
        clip.setAttribute("aria-pressed", String(k === i));
      });
      load(frames[i]);
      if (!still) preload(i + 1);
      frames.forEach((img, k) => img.classList.toggle("is-on", k === i));
      const d = clips[i].dataset;
      set("[data-show-name]", d.name);
      set("[data-show-time]", d.time);
      set("[data-show-scope]", d.scope);
      set("[data-show-prompt]", d.prompt);
      set("[data-show-cmd]", d.cmd);
      set("[data-show-out]", d.out);
      show.style.setProperty("--ph", `${(i + 0.5) * 25}%`);
    };
    let timer = still ? 0 : setInterval(() => pick((at + 1) % clips.length), 5000);
    clips.forEach((clip, i) =>
      clip.addEventListener("click", () => {
        clearInterval(timer);
        timer = 0;
        pick(i);
      })
    );
    pick(0);
  }
  // Copy button: a tint on success; otherwise the command is selected for Ctrl+C.
  for (const btn of document.querySelectorAll("[data-copy]")) {
    const label = btn.getAttribute("aria-label") ?? "";
    btn.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(btn.dataset.copy ?? "");
        btn.classList.add("copied");
        btn.setAttribute("aria-label", "Copied");
      } catch {
        const code = btn.closest("[data-copy-box]")?.querySelector("code");
        if (code) getSelection()?.selectAllChildren(code);
        btn.setAttribute("aria-label", "Selected: press Ctrl+C");
      }
      setTimeout(() => {
        btn.classList.remove("copied");
        btn.setAttribute("aria-label", label);
      }, 1800);
    });
  }

  // Reveals: each [data-reveal] fades up once as it enters the view.
  const reveals = document.querySelectorAll("[data-reveal]");
  if (still || !("IntersectionObserver" in window)) {
    reveals.forEach(el => el.classList.add("in"));
  } else {
    const io = new IntersectionObserver(
      entries => {
        for (const e of entries) {
          if (!e.isIntersecting) continue;
          e.target.classList.add("in");
          io.unobserve(e.target);
        }
      },
      { rootMargin: "0px 0px -8% 0px", threshold: 0.12 }
    );
    reveals.forEach(el => io.observe(el));
  }

  // Export race: lanes fill at 20 times the measured speed once the card is in view.
  const race = document.querySelector("[data-race]");
  if (race && !still && "IntersectionObserver" in window) {
    const lanes = [...race.querySelectorAll(".lane")].map(el => ({
      el,
      s: Number(el.style.getPropertyValue("--s")),
      time: el.querySelector("[data-race-time]"),
    }));
    let start = 0;
    const frame = now => {
      const t = ((now - start) / 1000) * 20;
      let running = false;
      for (const lane of lanes) {
        const p = Math.min(t, lane.s);
        running ||= p < lane.s;
        lane.el.style.setProperty("--p", p.toFixed(3));
        lane.el.classList.toggle("done", p >= lane.s);
        lane.time.textContent = `${p.toFixed(1)} s`;
      }
      if (running) requestAnimationFrame(frame);
    };
    const run = () => {
      start = performance.now();
      requestAnimationFrame(frame);
    };
    for (const lane of lanes) lane.el.style.setProperty("--p", "0");
    new IntersectionObserver(
      (entries, io) => {
        if (!entries.some(e => e.isIntersecting)) return;
        io.disconnect();
        run();
      },
      { threshold: 0.5 }
    ).observe(race);
    race.querySelector("[data-race-replay]")?.addEventListener("click", run);
  } else {
    race?.querySelector("[data-race-replay]")?.remove();
  }

  // Page timeline: each [data-scene] section is a clip as wide as its share of the page. The
  // playhead marks the point the viewport has reached: the top at first, the bottom at the end.
  const scrub = document.querySelector("[data-scrub]");
  const scenes = [...document.querySelectorAll("[data-scene]")];
  const clips = scrub ? [...scrub.querySelectorAll(".scrub-clips li")] : [];
  const tc = scrub?.querySelector("[data-scrub-tc]");
  const sceneName = scrub?.querySelector("[data-scrub-scene]");
  let starts = [];
  const measure = () => {
    const page = document.documentElement.scrollHeight;
    starts = scenes.map((s, i) => (i === 0 ? 0 : s.getBoundingClientRect().top + scrollY));
    starts.forEach((top, i) => {
      const end = i + 1 < starts.length ? starts[i + 1] : page;
      clips[i]?.style.setProperty("flex-grow", String(Math.max(1, end - top)));
    });
  };
  const pad = n => String(Math.floor(n)).padStart(2, "0");

  // Scroll-linked values, written once per frame.
  const header = document.querySelector(".site-header");
  const showcase = document.querySelector("[data-showcase]");
  const floats = [...document.querySelectorAll("[data-float]")];
  let queued = false;
  const update = () => {
    queued = false;
    const y = scrollY;
    header?.toggleAttribute("data-scrolled", y > 8);
    const page = document.documentElement.scrollHeight;
    const progress = Math.min(1, Math.max(0, y / Math.max(1, page - innerHeight)));
    if (scrub) {
      const at = y + innerHeight * progress;
      let current = 0;
      starts.forEach((top, i) => {
        if (at >= top) current = i;
      });
      clips.forEach((clip, i) => clip.classList.toggle("is-on", i === current));
      // Clips are at least as wide as their labels, so the playhead is placed inside the
      // current clip rather than at the page's overall progress.
      const end = starts[current + 1] ?? page;
      const within = Math.min(1, (at - starts[current]) / Math.max(1, end - starts[current]));
      const clip = clips[current];
      if (clip) scrub.style.setProperty("--x", (clip.offsetLeft + within * clip.offsetWidth).toFixed(1) + "px");
      scrub.toggleAttribute("data-show", y > innerHeight * 0.35);
      // The page plays as one minute at 30 frames per second.
      const seconds = progress * 60;
      tc.textContent = `${pad(seconds / 60)}:${pad(seconds % 60)}:${pad((seconds * 30) % 30)}`;
      sceneName.textContent = scenes[current]?.dataset.scene ?? "";
    }
    if (still) return;
    // The sky goes from dawn (top) through noon to dusk (bottom); the sun crosses it.
    root.style.setProperty("--a", (Math.min(1, progress * 2.5) * 100).toFixed(1) + "%");
    root.style.setProperty("--b", (Math.max(0, progress * 1.6 - 0.6) * 100).toFixed(1) + "%");
    root.style.setProperty("--sun-x", (90 - progress * 80).toFixed(2) + "%");
    root.style.setProperty("--sun-y", (6 + 30 * (2 * progress - 1) ** 2).toFixed(2) + "%");
    // The far clouds move at a third of the page's speed.
    document.body.style.setProperty("--sy", String(Math.round(y * -0.33)));
    // Hero cards drift apart as the hero scrolls away.
    const p = Math.min(1, y / innerHeight);
    for (const el of floats) el.style.setProperty("--f", String(p * Number(el.dataset.float)));
    // The showcase tilts upright between entering the view and reaching its upper third.
    if (showcase) {
      const top = showcase.getBoundingClientRect().top;
      const t = Math.min(1, Math.max(0, (innerHeight - top) / (innerHeight * 0.65)));
      showcase.style.setProperty("--t", t.toFixed(3));
    }
  };
  addEventListener(
    "scroll",
    () => {
      if (!queued) {
        queued = true;
        requestAnimationFrame(update);
      }
    },
    { passive: true }
  );
  addEventListener("resize", () => {
    measure();
    update();
  });
  // Lazy images change the page's height as they load.
  addEventListener("load", () => {
    measure();
    update();
  });
  // The first measurement waits for a frame so it does not force a layout before first paint.
  requestAnimationFrame(() => {
    measure();
    update();
  });
})();
